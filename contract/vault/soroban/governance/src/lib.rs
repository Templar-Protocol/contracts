#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod types;
pub use types::*;

use alloc::{string::String as AllocString, vec::Vec as AllocVec};
use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contractimpl, Address, Bytes, BytesN, Env, Executable, IntoVal, String, Symbol, Vec,
};
use templar_curator_primitives::governance::{
    timelock_config_decision, CapChangeError, FeeChangeError, FeeConfig, MembershipChangeError,
    PendingActions, PendingValue, RelativeCapChangeError, Restrictions as GovernanceRestrictions,
    TakePending, TimelockConfigError, TimelockDecision,
};
use templar_curator_primitives::{nonnegative_i128_to_u128, seconds_to_nanoseconds};
use templar_soroban_shared_types::{
    GovernanceCommand, VaultCommand, GOVERNANCE_CONFIG_KIND_ALLOCATORS,
    GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS, GOVERNANCE_CONFIG_KIND_CURATOR,
    GOVERNANCE_CONFIG_KIND_GOVERNANCE, GOVERNANCE_CONFIG_KIND_IDLE_RESYNC_COOLDOWN,
    GOVERNANCE_CONFIG_KIND_SENTINEL, GOVERNANCE_CONFIG_KIND_SKIM_RECIPIENT,
    GOVERNANCE_CONFIG_KIND_WITHDRAWAL_COOLDOWN, GOVERNANCE_POLICY_KIND_CAP,
    GOVERNANCE_POLICY_KIND_FEES, GOVERNANCE_POLICY_KIND_GROUP, GOVERNANCE_POLICY_KIND_PAUSED,
    GOVERNANCE_POLICY_KIND_REMOVE_MARKET, GOVERNANCE_POLICY_KIND_RESTRICTIONS,
    GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
};
use templar_vault_kernel::math::wad::Wad;
use templar_vault_kernel::{DurationNs, TimestampNs, DEFAULT_COOLDOWN_NS};

const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
const INSTANCE_TTL_EXTEND_TO: u32 = 3_110_400;
const MIN_TIMELOCK_NS: u64 = 0;
const DAY_NS: u64 = 86_400_000_000_000;
const MAX_TIMELOCK_NS: u64 = 30 * DAY_NS;
const MAX_PENDING_PROPOSALS: usize = 64;
const PENDING_PAGE_SIZE: u64 = 16;
const DEFAULT_WITHDRAWAL_COOLDOWN_NS: u64 = DEFAULT_COOLDOWN_NS;
const DEFAULT_IDLE_RESYNC_COOLDOWN_NS: u64 = 120 * 1_000_000_000;

#[derive(Clone, Copy, Eq, PartialEq)]
enum RevokerRole {
    Admin,
    Sentinel,
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
enum ProposalKey {
    ProposalId(u64),
    #[allow(dead_code)]
    Action(GovernanceActionKey),
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
enum GovernanceActionKey {
    Admin,
    Pause,
    Curator,
    Governance,
    SupplyQueue,
    Fees,
    WithdrawalCooldown,
    IdleResyncCooldown,
    Restrictions,
    Sentinel,
    Allocators,
    AllowedAdapters,
    Cap(u32),
    MarketRemoval(u32),
    CapGroupCap(String),
    CapGroupRelativeCap(String),
    CapGroupMembership(u32),
    SkimRecipient,
    Skim(Address),
    Upgrade,
    Migrate,
    CancelMigration,
    TimelockConfig(TimelockKind),
    Other(Symbol, BytesN<32>),
}

impl GovernanceAction {
    fn pending_key(&self) -> GovernanceActionKey {
        match self {
            Self::SetAdmin(_) => GovernanceActionKey::Admin,
            Self::SetPaused(_) => GovernanceActionKey::Pause,
            Self::SetCurator(_) => GovernanceActionKey::Curator,
            Self::SetGovernance(_) => GovernanceActionKey::Governance,
            Self::SetSupplyQueue(_, _) => GovernanceActionKey::SupplyQueue,
            Self::SetFees(_) => GovernanceActionKey::Fees,
            Self::SetWithdrawalCooldown(_) => GovernanceActionKey::WithdrawalCooldown,
            Self::SetIdleResyncCooldown(_) => GovernanceActionKey::IdleResyncCooldown,
            Self::SetRestrictions(_, _) => GovernanceActionKey::Restrictions,
            Self::SetSentinel(_) => GovernanceActionKey::Sentinel,
            Self::SetAllocators(_) => GovernanceActionKey::Allocators,
            Self::SetAllowedAdapters(_) => GovernanceActionKey::AllowedAdapters,
            Self::SetCap(market_id, _) => GovernanceActionKey::Cap(*market_id),
            Self::RemoveMarket(market_id) => GovernanceActionKey::MarketRemoval(*market_id),
            Self::SetGroupCap(group, _) => GovernanceActionKey::CapGroupCap(group.clone()),
            Self::SetGroupRelCap(group, _) => {
                GovernanceActionKey::CapGroupRelativeCap(group.clone())
            }
            Self::SetGroupMember(market_id, _) => {
                GovernanceActionKey::CapGroupMembership(*market_id)
            }
            Self::SetSkimRecipient(_) => GovernanceActionKey::SkimRecipient,
            Self::Skim(account) => GovernanceActionKey::Skim(account.clone()),
            Self::Upgrade(_) => GovernanceActionKey::Upgrade,
            Self::Migrate => GovernanceActionKey::Migrate,
            Self::CancelMigration => GovernanceActionKey::CancelMigration,
            Self::SetTimelock(kind, _) => GovernanceActionKey::TimelockConfig(*kind),
            Self::Other(key, payload_hash) => {
                GovernanceActionKey::Other(key.clone(), payload_hash.clone())
            }
        }
    }
}

impl QueuedProposal {
    fn action_key(&self) -> GovernanceActionKey {
        self.action.pending_key()
    }
}

#[contract]
pub struct SorobanVaultGovernanceContract;

#[contractimpl]
impl SorobanVaultGovernanceContract {
    pub fn __constructor(
        env: Env,
        admin: Address,
        vault: Address,
        timelock_ns: u64,
    ) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        require_contract_address(&vault)?;
        require_constructor_topology(&env, &admin, &vault)?;
        validate_timelock_ns(timelock_ns)?;

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Vault, &vault);
        env.storage()
            .instance()
            .set(&DataKey::TimelockNs, &timelock_ns);
        env.storage()
            .instance()
            .set(&DataKey::Timelocks, &Timelocks::from_default(timelock_ns));
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &1u64);
        env.storage()
            .instance()
            .set(&DataKey::CurrentPaused, &false);
        env.storage().instance().set(
            &DataKey::CurrentFees,
            &FeeParams {
                performance_fee_wad: 0,
                performance_recipient: admin.clone(),
                management_fee_wad: 0,
                management_recipient: admin,
                max_growth_rate_wad: None,
            },
        );
        env.storage().instance().set(
            &DataKey::CurrentWithdrawalCooldownNs,
            &DEFAULT_WITHDRAWAL_COOLDOWN_NS,
        );
        env.storage().instance().set(
            &DataKey::CurrentIdleResyncCooldownNs,
            &DEFAULT_IDLE_RESYNC_COOLDOWN_NS,
        );
        env.storage()
            .instance()
            .set(&DataKey::CurrentRestrictionMode, &RestrictionMode::None);
        env.storage().instance().set(
            &DataKey::CurrentRestrictionAccounts,
            &Vec::<Address>::new(&env),
        );
        Ok(())
    }

    pub fn submit_set_paused(
        env: Env,
        caller: Address,
        paused: bool,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetPaused(paused))
    }

    pub fn submit_set_admin(
        env: Env,
        caller: Address,
        new_admin: Address,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetAdmin(new_admin))
    }

    pub fn submit_set_curator(
        env: Env,
        caller: Address,
        new_curator: Address,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetCurator(new_curator))
    }

    pub fn submit_set_governance(
        env: Env,
        caller: Address,
        governance: Address,
    ) -> Result<u64, GovernanceError> {
        require_governance_target(&env, &governance)?;
        Self::submit(env, caller, GovernanceAction::SetGovernance(governance))
    }

    pub fn submit_set_supply_queue(
        env: Env,
        caller: Address,
        target_ids: Vec<u32>,
        adapters: Vec<Address>,
    ) -> Result<u64, GovernanceError> {
        Self::submit(
            env,
            caller,
            GovernanceAction::SetSupplyQueue(target_ids, adapters),
        )
    }

    pub fn submit_set_fees(
        env: Env,
        caller: Address,
        performance_fee_wad: i128,
        performance_recipient: Address,
        management_fee_wad: i128,
        management_recipient: Address,
        max_growth_rate_wad: Option<i128>,
    ) -> Result<u64, GovernanceError> {
        let params = FeeParams {
            performance_fee_wad,
            performance_recipient,
            management_fee_wad,
            management_recipient,
            max_growth_rate_wad,
        };
        Self::submit(env, caller, GovernanceAction::SetFees(params))
    }

    pub fn submit_set_withdrawal_cooldown(
        env: Env,
        caller: Address,
        withdrawal_cooldown_ns: u64,
    ) -> Result<u64, GovernanceError> {
        Self::submit(
            env,
            caller,
            GovernanceAction::SetWithdrawalCooldown(withdrawal_cooldown_ns),
        )
    }

    pub fn submit_set_idle_resync_cooldown(
        env: Env,
        caller: Address,
        idle_resync_cooldown_ns: u64,
    ) -> Result<u64, GovernanceError> {
        Self::submit(
            env,
            caller,
            GovernanceAction::SetIdleResyncCooldown(idle_resync_cooldown_ns),
        )
    }

    pub fn submit_set_restrictions(
        env: Env,
        caller: Address,
        mode: u32,
        accounts: Vec<Address>,
    ) -> Result<u64, GovernanceError> {
        let mode = RestrictionMode::from_u32(mode)?;
        Self::submit(
            env,
            caller,
            GovernanceAction::SetRestrictions(mode, accounts),
        )
    }

    pub fn set_paused(env: Env, caller: Address, paused: bool) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        require_sentinel(&env, &caller)?;
        if !paused {
            return Err(GovernanceError::InvalidInput);
        }

        let action = GovernanceAction::SetPaused(paused);
        require_not_abdicated(&env, &action)?;
        let vault = get_address(&env, DataKey::Vault)?;
        execute_vault_governance_action_as_caller(&env, &vault, &caller, &action)?;
        env.storage()
            .instance()
            .set(&DataKey::CurrentPaused, &paused);
        Ok(())
    }

    pub fn set_restrictions(
        env: Env,
        caller: Address,
        mode: u32,
        accounts: Vec<Address>,
    ) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        require_sentinel(&env, &caller)?;
        let mode = RestrictionMode::from_u32(mode)?;
        let action = GovernanceAction::SetRestrictions(mode, accounts.clone());
        require_not_abdicated(&env, &action)?;
        if restrictions_change_is_relaxed(&env, mode, &accounts) {
            return Err(GovernanceError::InvalidInput);
        }
        let vault = get_address(&env, DataKey::Vault)?;
        execute_vault_governance_action_as_caller(&env, &vault, &caller, &action)?;
        env.storage()
            .instance()
            .set(&DataKey::CurrentRestrictionMode, &mode);
        env.storage()
            .instance()
            .set(&DataKey::CurrentRestrictionAccounts, &accounts);
        Ok(())
    }

    pub fn submit_set_sentinel(
        env: Env,
        caller: Address,
        sentinel: Address,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetSentinel(sentinel))
    }

    pub fn submit_set_allocators(
        env: Env,
        caller: Address,
        allocators: Vec<Address>,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetAllocators(allocators))
    }

    pub fn submit_set_allowed_adapters(
        env: Env,
        caller: Address,
        adapters: Vec<Address>,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetAllowedAdapters(adapters))
    }

    pub fn submit_set_timelock(
        env: Env,
        caller: Address,
        kind: TimelockKind,
        new_timelock_ns: u64,
    ) -> Result<u64, GovernanceError> {
        Self::submit(
            env,
            caller,
            GovernanceAction::SetTimelock(kind, new_timelock_ns),
        )
    }

    pub fn submit_set_cap(
        env: Env,
        caller: Address,
        market_id: u32,
        new_cap: i128,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetCap(market_id, new_cap))
    }

    pub fn submit_remove_market(
        env: Env,
        caller: Address,
        market_id: u32,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::RemoveMarket(market_id))
    }

    pub fn submit_set_group_cap(
        env: Env,
        caller: Address,
        cap_group_id: String,
        new_cap: i128,
    ) -> Result<u64, GovernanceError> {
        Self::submit(
            env,
            caller,
            GovernanceAction::SetGroupCap(cap_group_id, new_cap),
        )
    }

    pub fn submit_set_group_rel_cap(
        env: Env,
        caller: Address,
        cap_group_id: String,
        new_relative_cap_wad: i128,
    ) -> Result<u64, GovernanceError> {
        Self::submit(
            env,
            caller,
            GovernanceAction::SetGroupRelCap(cap_group_id, new_relative_cap_wad),
        )
    }

    pub fn submit_set_group_member(
        env: Env,
        caller: Address,
        market_id: u32,
        cap_group_id: String,
    ) -> Result<u64, GovernanceError> {
        Self::submit(
            env,
            caller,
            GovernanceAction::SetGroupMember(market_id, cap_group_id),
        )
    }

    pub fn submit_set_skim_recipient(
        env: Env,
        caller: Address,
        recipient: Address,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetSkimRecipient(recipient))
    }

    pub fn submit_skim(env: Env, caller: Address, token: Address) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::Skim(token))
    }

    pub fn submit_upgrade(
        env: Env,
        caller: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::Upgrade(new_wasm_hash))
    }

    pub fn submit_migrate(env: Env, caller: Address) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::Migrate)
    }

    pub fn submit_cancel_migration(env: Env, caller: Address) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::CancelMigration)
    }

    pub fn abdicate(
        env: Env,
        caller: Address,
        kind: GovernanceActionKind,
    ) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        env.storage()
            .instance()
            .set(&DataKey::Abdicated(kind), &true);
        Ok(())
    }

    pub fn is_abdicated(env: Env, kind: GovernanceActionKind) -> bool {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::Abdicated(kind))
            .unwrap_or(false)
    }

    pub fn submit_other(
        env: Env,
        caller: Address,
        key: Symbol,
        payload_hash: BytesN<32>,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::Other(key, payload_hash))
    }

    pub fn check_other(env: Env, key: Symbol, payload_hash: BytesN<32>) -> bool {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::ApprovedOther(key, payload_hash))
            .unwrap_or(false)
    }

    pub fn consume_other(
        env: Env,
        caller: Address,
        key: Symbol,
        payload_hash: BytesN<32>,
    ) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        require_vault_caller(&env, &caller)?;

        let approved: bool = env
            .storage()
            .instance()
            .get(&DataKey::ApprovedOther(key.clone(), payload_hash.clone()))
            .unwrap_or(false);
        if !approved {
            return Err(GovernanceError::OtherNotApproved);
        }

        env.storage()
            .instance()
            .remove(&DataKey::ApprovedOther(key, payload_hash));
        Ok(())
    }

    pub fn revoke_other_pending(
        env: Env,
        caller: Address,
        key: Symbol,
        payload_hash: BytesN<32>,
    ) -> Result<u32, GovernanceError> {
        extend_instance_ttl(&env);
        require_revoke_kind(&env, &caller, GovernanceActionKind::Other)?;
        let key_for_match = key.clone();
        let hash_for_match = payload_hash.clone();
        let removed = revoke_by_action_key(
            &env,
            &GovernanceActionKey::Other(key_for_match, hash_for_match),
        );
        if removed == 0 {
            return Err(GovernanceError::ProposalNotFound);
        }
        Ok(removed)
    }

    pub fn accept(env: Env, caller: Address, proposal_id: u64) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;

        let now_ns = ledger_timestamp_ns(&env)?;
        let mut queue = load_queue(&env);
        let proposal = match queue.take_by_key(
            now_ns,
            &ProposalKey::ProposalId(proposal_id),
            queued_proposal_key_by_id,
        ) {
            TakePending::Ready(proposal) => proposal,
            TakePending::Missing => return Err(GovernanceError::ProposalNotFound),
            TakePending::Pending { .. } => return Err(GovernanceError::ProposalNotMature),
        };

        execute_action(&env, &proposal.action)?;
        save_queue(&env, &queue);
        ProposalAccepted { id: proposal_id }.publish(&env);
        Ok(())
    }

    pub fn accept_kind(
        env: Env,
        caller: Address,
        kind: GovernanceActionKind,
    ) -> Result<u64, GovernanceError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        let now_ns = ledger_timestamp_ns(&env)?;

        let mut queue = load_queue(&env);
        let mut matching = 0u32;
        for entry in queue.iter() {
            if action_kind(&entry.value.action) == kind {
                matching = matching
                    .checked_add(1)
                    .ok_or(GovernanceError::ArithmeticOverflow)?;
            }
        }
        if matching > 1 {
            return Err(GovernanceError::DuplicatePending);
        }

        let proposal = match queue.take_by_key(now_ns, &kind, queued_proposal_kind) {
            TakePending::Ready(proposal) => proposal,
            TakePending::Missing => return Err(GovernanceError::ProposalNotFound),
            TakePending::Pending { .. } => return Err(GovernanceError::ProposalNotMature),
        };

        execute_action(&env, &proposal.action)?;
        save_queue(&env, &queue);
        ProposalAccepted { id: proposal.id }.publish(&env);
        Ok(proposal.id)
    }

    pub fn revoke(env: Env, caller: Address, proposal_id: u64) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        let mut queue = load_queue(&env);
        let kind = queue
            .iter()
            .find(|entry| entry.value.id == proposal_id)
            .map(|entry| action_kind(&entry.value.action))
            .ok_or(GovernanceError::ProposalNotFound)?;
        require_revoke_kind(&env, &caller, kind)?;

        if queue
            .revoke_by_key(
                &ProposalKey::ProposalId(proposal_id),
                queued_proposal_key_by_id,
            )
            .is_empty()
        {
            return Err(GovernanceError::ProposalNotFound);
        }
        save_queue(&env, &queue);
        ProposalRevoked { id: proposal_id }.publish(&env);
        Ok(())
    }

    pub fn revoke_kind(
        env: Env,
        caller: Address,
        kind: GovernanceActionKind,
    ) -> Result<u32, GovernanceError> {
        extend_instance_ttl(&env);
        require_revoke_kind(&env, &caller, kind)?;
        let mut queue = load_queue(&env);
        let mut matching = 0u32;
        for entry in queue.iter() {
            if action_kind(&entry.value.action) == kind {
                matching = matching
                    .checked_add(1)
                    .ok_or(GovernanceError::ArithmeticOverflow)?;
            }
        }
        if matching == 0 {
            return Err(GovernanceError::ProposalNotFound);
        }
        if matching > 1 {
            return Err(GovernanceError::DuplicatePending);
        }

        let removed = queue.revoke_by_key(&kind, queued_proposal_kind);
        save_queue(&env, &queue);
        let Some(proposal) = removed.first() else {
            return Err(GovernanceError::ProposalNotFound);
        };
        ProposalRevoked { id: proposal.id }.publish(&env);
        Ok(1)
    }

    pub fn pending(env: Env, proposal_id: u64) -> Result<PendingProposal, GovernanceError> {
        extend_instance_ttl(&env);
        load_proposal(&env, proposal_id)
    }

    pub fn pending_ids(env: Env) -> Vec<u64> {
        extend_instance_ttl(&env);
        load_pending_ids(&env)
    }

    pub fn timelock_ns(env: Env, kind: TimelockKind) -> u64 {
        extend_instance_ttl(&env);
        load_timelocks(&env).get(kind)
    }

    pub fn timelocks(env: Env) -> Timelocks {
        extend_instance_ttl(&env);
        load_timelocks(&env)
    }

    pub fn admin(env: Env) -> Result<Address, GovernanceError> {
        extend_instance_ttl(&env);
        get_address(&env, DataKey::Admin)
    }

    pub fn vault(env: Env) -> Result<Address, GovernanceError> {
        extend_instance_ttl(&env);
        get_address(&env, DataKey::Vault)
    }

    pub fn sentinel(env: Env) -> Option<Address> {
        extend_instance_ttl(&env);
        env.storage().instance().get(&DataKey::Sentinel)
    }

    pub fn extend_ttl(env: Env, caller: Address) -> Result<(), GovernanceError> {
        require_admin(&env, &caller)?;
        extend_instance_ttl(&env);
        extend_pending_queue_ttl(&env);
        Ok(())
    }

    fn submit(env: Env, caller: Address, action: GovernanceAction) -> Result<u64, GovernanceError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        require_not_abdicated(&env, &action)?;
        validate_action(&env, &action)?;

        let decision = decide_submission(&env, &action)?;
        let now_ns = ledger_timestamp_ns(&env)?;
        let timelock_ns = DurationNs(load_timelocks(&env).get(timelock_kind_for_action(&action)));
        let pending_key = action.pending_key();
        let mut queue = load_queue(&env);
        let replaces_existing = queue.has_pending_key(&pending_key, QueuedProposal::action_key);
        if matches!(decision, TimelockDecision::Timelocked)
            && !replaces_existing
            && queue.len() >= MAX_PENDING_PROPOSALS
        {
            return Err(GovernanceError::InvalidInput);
        }

        let id = next_proposal_id(&env)?;

        if matches!(decision, TimelockDecision::Immediate) {
            let replaced = queue.revoke_by_key(&pending_key, QueuedProposal::action_key);
            if !replaced.is_empty() {
                save_queue(&env, &queue);
            }
            for proposal in replaced.iter() {
                ProposalRevoked { id: proposal.id }.publish(&env);
            }
            execute_action(&env, &action)?;
            ProposalSubmitted {
                id,
                valid_after_ns: 0,
            }
            .publish(&env);
            ProposalAccepted { id }.publish(&env);
            return Ok(id);
        }

        let scheduled = queue.schedule_replacing(
            &pending_key,
            QueuedProposal::action_key,
            QueuedProposal {
                id,
                action: action.clone(),
            },
            now_ns,
            timelock_ns,
        );
        save_queue(&env, &queue);

        for replaced in scheduled.replaced.iter() {
            ProposalRevoked { id: replaced.id }.publish(&env);
        }

        let valid_after_ns = scheduled.ready_at_ns;

        ProposalSubmitted {
            id,
            valid_after_ns: valid_after_ns.into(),
        }
        .publish(&env);
        Ok(id)
    }
}

fn action_kind(action: &GovernanceAction) -> GovernanceActionKind {
    match action {
        GovernanceAction::SetAdmin(_) => GovernanceActionKind::Admin,
        GovernanceAction::SetPaused(_) => GovernanceActionKind::Pause,
        GovernanceAction::SetCurator(_) => GovernanceActionKind::Curator,
        GovernanceAction::SetGovernance(_) => GovernanceActionKind::Governance,
        GovernanceAction::SetSupplyQueue(_, _) => GovernanceActionKind::SupplyQueue,
        GovernanceAction::SetFees(_) => GovernanceActionKind::Fees,
        GovernanceAction::SetWithdrawalCooldown(_) => GovernanceActionKind::WithdrawalCooldown,
        GovernanceAction::SetIdleResyncCooldown(_) => GovernanceActionKind::IdleResyncCooldown,
        GovernanceAction::SetRestrictions(_, _) => GovernanceActionKind::Restrictions,
        GovernanceAction::SetSentinel(_) => GovernanceActionKind::Sentinel,
        GovernanceAction::SetAllocators(_) => GovernanceActionKind::Allocators,
        GovernanceAction::SetAllowedAdapters(_) => GovernanceActionKind::AllowedAdapters,
        GovernanceAction::SetCap(_, _) => GovernanceActionKind::Cap,
        GovernanceAction::RemoveMarket(_) => GovernanceActionKind::MarketRemoval,
        GovernanceAction::SetGroupCap(_, _)
        | GovernanceAction::SetGroupRelCap(_, _)
        | GovernanceAction::SetGroupMember(_, _) => GovernanceActionKind::CapGroup,
        GovernanceAction::SetSkimRecipient(_) | GovernanceAction::Skim(_) => {
            GovernanceActionKind::Skim
        }
        GovernanceAction::Upgrade(_) => GovernanceActionKind::Upgrade,
        GovernanceAction::Migrate => GovernanceActionKind::Migrate,
        GovernanceAction::CancelMigration => GovernanceActionKind::CancelMigration,
        GovernanceAction::SetTimelock(_, _) => GovernanceActionKind::TimelockConfig,
        GovernanceAction::Other(_, _) => GovernanceActionKind::Other,
    }
}

fn timelock_kind_for_action(action: &GovernanceAction) -> TimelockKind {
    match action {
        GovernanceAction::SetAdmin(_) => TimelockKind::Admin,
        GovernanceAction::SetPaused(_) => TimelockKind::Pause,
        GovernanceAction::SetCurator(_) => TimelockKind::Curator,
        GovernanceAction::SetGovernance(_) => TimelockKind::Governance,
        GovernanceAction::SetSupplyQueue(_, _) => TimelockKind::SupplyQueue,
        GovernanceAction::SetFees(_)
        | GovernanceAction::SetWithdrawalCooldown(_)
        | GovernanceAction::SetIdleResyncCooldown(_) => TimelockKind::Fees,
        GovernanceAction::SetRestrictions(_, _) => TimelockKind::Restrictions,
        GovernanceAction::SetSentinel(_) => TimelockKind::Sentinel,
        GovernanceAction::SetAllocators(_) => TimelockKind::Allocators,
        GovernanceAction::SetAllowedAdapters(_) => TimelockKind::AllowedAdapters,
        GovernanceAction::SetCap(_, _) => TimelockKind::Cap,
        GovernanceAction::RemoveMarket(_) => TimelockKind::MarketRemoval,
        GovernanceAction::SetGroupCap(_, _)
        | GovernanceAction::SetGroupRelCap(_, _)
        | GovernanceAction::SetGroupMember(_, _) => TimelockKind::CapGroup,
        GovernanceAction::SetSkimRecipient(_) | GovernanceAction::Skim(_) => TimelockKind::Skim,
        GovernanceAction::Upgrade(_) => TimelockKind::Upgrade,
        GovernanceAction::Migrate | GovernanceAction::CancelMigration => TimelockKind::Migration,
        GovernanceAction::SetTimelock(_, _) => TimelockKind::TimelockConfig,
        GovernanceAction::Other(_, _) => TimelockKind::Other,
    }
}

fn require_unique_target_ids(target_ids: &Vec<u32>) -> Result<(), GovernanceError> {
    for i in 0..target_ids.len() {
        let target_id = target_ids.get_unchecked(i);
        for j in (i + 1)..target_ids.len() {
            if target_id == target_ids.get_unchecked(j) {
                return Err(GovernanceError::InvalidInput);
            }
        }
    }
    Ok(())
}

fn validate_action(env: &Env, action: &GovernanceAction) -> Result<(), GovernanceError> {
    match action {
        GovernanceAction::SetGovernance(governance) => require_governance_target(env, governance),
        GovernanceAction::SetSupplyQueue(target_ids, adapters) => {
            require_unique_target_ids(target_ids)?;
            if !adapters.is_empty() && adapters.len() != target_ids.len() {
                return Err(GovernanceError::InvalidInput);
            }
            for adapter in adapters.iter() {
                require_contract_address(&adapter)?;
            }
            Ok(())
        }
        GovernanceAction::SetFees(params) => {
            let _ = to_wad(params.performance_fee_wad)?;
            let _ = to_wad(params.management_fee_wad)?;
            if let Some(max_rate) = params.max_growth_rate_wad {
                let _ = to_wad(max_rate)?;
            }
            Ok(())
        }
        GovernanceAction::SetCap(_, new_cap) | GovernanceAction::SetGroupCap(_, new_cap) => {
            let _ = to_wad(*new_cap)?;
            Ok(())
        }
        GovernanceAction::SetGroupRelCap(_, new_relative_cap_wad) => {
            let relative = to_wad(*new_relative_cap_wad)?;
            if relative > Wad::one() {
                return Err(GovernanceError::InvalidInput);
            }
            Ok(())
        }
        GovernanceAction::SetTimelock(_, new_timelock_ns) => validate_timelock_ns(*new_timelock_ns),
        GovernanceAction::Other(_, _) => Ok(()),
        _ => Ok(()),
    }
}

fn cap_to_u128(value: i128) -> Result<u128, GovernanceError> {
    nonnegative_i128_to_u128(value).ok_or(GovernanceError::InvalidInput)
}

#[allow(clippy::too_many_lines)]
fn decide_submission(
    env: &Env,
    action: &GovernanceAction,
) -> Result<TimelockDecision, GovernanceError> {
    match action {
        GovernanceAction::SetAdmin(new_admin) => {
            let current = get_address(env, DataKey::Admin)?;
            if &current == new_admin {
                return Err(GovernanceError::NoChange);
            }
            Ok(TimelockDecision::Timelocked)
        }
        GovernanceAction::SetPaused(paused) => {
            if *paused {
                return Err(GovernanceError::InvalidInput);
            }

            let current = env
                .storage()
                .instance()
                .get(&DataKey::CurrentPaused)
                .unwrap_or(false);
            if *paused == current {
                return Err(GovernanceError::NoChange);
            }

            Ok(TimelockDecision::Timelocked)
        }
        GovernanceAction::SetSentinel(next) => {
            let current: Option<Address> = env.storage().instance().get(&DataKey::Sentinel);
            if current.as_ref() == Some(next) {
                return Err(GovernanceError::NoChange);
            }
            Ok(TimelockDecision::from_requires_timelock(current.is_some()))
        }
        GovernanceAction::SetTimelock(kind, proposed) => {
            let current = load_timelocks(env).get(*kind);
            timelock_config_decision(
                DurationNs(current),
                DurationNs(*proposed),
                DurationNs(MIN_TIMELOCK_NS),
                DurationNs(MAX_TIMELOCK_NS),
            )
            .map_err(|err| match err {
                TimelockConfigError::NoChange => GovernanceError::NoChange,
                TimelockConfigError::OutOfBounds => GovernanceError::TimelockOutOfBounds,
            })
        }
        GovernanceAction::SetFees(proposed) => {
            let current: FeeParams = env
                .storage()
                .instance()
                .get(&DataKey::CurrentFees)
                .ok_or(GovernanceError::MissingConfig)?;

            let current_cfg = FeeConfig {
                performance_fee: to_wad(current.performance_fee_wad)?,
                management_fee: to_wad(current.management_fee_wad)?,
                performance_recipient: &current.performance_recipient,
                management_recipient: &current.management_recipient,
                max_rate: to_optional_wad(current.max_growth_rate_wad)?,
            };
            let proposed_cfg = FeeConfig {
                performance_fee: to_wad(proposed.performance_fee_wad)?,
                management_fee: to_wad(proposed.management_fee_wad)?,
                performance_recipient: &proposed.performance_recipient,
                management_recipient: &proposed.management_recipient,
                max_rate: to_optional_wad(proposed.max_growth_rate_wad)?,
            };
            let decision = FeeConfig::evaluate_change(&current_cfg, &proposed_cfg).map_err(
                |err| match err {
                    FeeChangeError::NoChange => GovernanceError::NoChange,
                    FeeChangeError::PerformanceFeeTooHigh
                    | FeeChangeError::ManagementFeeTooHigh => GovernanceError::InvalidInput,
                },
            )?;

            if decision.timelocked {
                Ok(TimelockDecision::Timelocked)
            } else {
                Ok(TimelockDecision::Immediate)
            }
        }
        GovernanceAction::SetWithdrawalCooldown(proposed) => {
            let current = env
                .storage()
                .instance()
                .get(&DataKey::CurrentWithdrawalCooldownNs)
                .unwrap_or(DEFAULT_WITHDRAWAL_COOLDOWN_NS);
            if current == *proposed {
                return Err(GovernanceError::NoChange);
            }
            Ok(TimelockDecision::Timelocked)
        }
        GovernanceAction::SetIdleResyncCooldown(proposed) => {
            let current = env
                .storage()
                .instance()
                .get(&DataKey::CurrentIdleResyncCooldownNs)
                .unwrap_or(DEFAULT_IDLE_RESYNC_COOLDOWN_NS);
            if current == *proposed {
                return Err(GovernanceError::NoChange);
            }
            Ok(TimelockDecision::Timelocked)
        }
        GovernanceAction::SetRestrictions(mode, accounts) => {
            let current_mode: RestrictionMode = env
                .storage()
                .instance()
                .get(&DataKey::CurrentRestrictionMode)
                .unwrap_or(RestrictionMode::None);
            let current_accounts: Vec<Address> = env
                .storage()
                .instance()
                .get(&DataKey::CurrentRestrictionAccounts)
                .unwrap_or_else(|| Vec::new(env));

            if current_mode == *mode && current_accounts == *accounts {
                return Err(GovernanceError::NoChange);
            }

            Ok(TimelockDecision::Timelocked)
        }
        GovernanceAction::SetCap(market_id, new_cap) => {
            let current: Option<i128> = env
                .storage()
                .instance()
                .get(&DataKey::CurrentCap(*market_id));
            let current = current.map(cap_to_u128).transpose()?;
            let decision = TimelockDecision::from_cap_change(current, cap_to_u128(*new_cap)?);
            match decision {
                Ok(TimelockDecision::Immediate) => Ok(TimelockDecision::Immediate),
                Ok(TimelockDecision::Timelocked) => Ok(TimelockDecision::Timelocked),
                Err(CapChangeError::NoChange) => Err(GovernanceError::NoChange),
            }
        }
        GovernanceAction::RemoveMarket(_) => Ok(TimelockDecision::from_requires_timelock(true)),
        GovernanceAction::SetGroupCap(cap_group_id, new_cap) => {
            let known: Option<bool> = env
                .storage()
                .instance()
                .get(&DataKey::KnownCapGroupCap(cap_group_id.clone()));
            if known != Some(true) {
                return Ok(TimelockDecision::Timelocked);
            }
            let current: Option<i128> = env
                .storage()
                .instance()
                .get(&DataKey::CurrentCapGroupCap(cap_group_id.clone()));
            let current = current.map(cap_to_u128).transpose()?;
            let decision =
                TimelockDecision::from_cap_group_cap_change(current, Some(cap_to_u128(*new_cap)?));
            match decision {
                Ok(TimelockDecision::Immediate) => Ok(TimelockDecision::Immediate),
                Ok(TimelockDecision::Timelocked) => Ok(TimelockDecision::Timelocked),
                Err(CapChangeError::NoChange) => Err(GovernanceError::NoChange),
            }
        }
        GovernanceAction::SetGroupRelCap(cap_group_id, new_relative_cap_wad) => {
            let known: Option<bool> = env
                .storage()
                .instance()
                .get(&DataKey::KnownCapGroupRelCap(cap_group_id.clone()));
            if known != Some(true) {
                return Ok(TimelockDecision::Timelocked);
            }
            let current: Option<i128> = env
                .storage()
                .instance()
                .get(&DataKey::CurrentCapGroupRelCap(cap_group_id.clone()));
            let current = current.map(to_wad).transpose()?;
            match TimelockDecision::from_relative_cap_change(
                current,
                Some(to_wad(*new_relative_cap_wad)?),
            ) {
                Ok(decision) => Ok(decision),
                Err(RelativeCapChangeError::NoChange) => Err(GovernanceError::NoChange),
                Err(RelativeCapChangeError::RelativeCapTooHigh) => {
                    Err(GovernanceError::InvalidInput)
                }
            }
        }
        GovernanceAction::SetGroupMember(market_id, cap_group_id) => {
            let known: Option<bool> = env
                .storage()
                .instance()
                .get(&DataKey::KnownCapGroupMembership(*market_id));
            if known != Some(true) {
                return Ok(TimelockDecision::Timelocked);
            }
            let current: Option<String> = env
                .storage()
                .instance()
                .get(&DataKey::CurrentCapGroupMembership(*market_id));
            let proposed = if cap_group_id.is_empty() {
                None
            } else {
                Some(cap_group_id)
            };
            match TimelockDecision::from_membership_assignment_change::<String>(
                current.as_ref(),
                proposed,
            ) {
                Ok(decision) => Ok(decision),
                Err(MembershipChangeError::NoChange) => Err(GovernanceError::NoChange),
            }
        }
        GovernanceAction::SetSkimRecipient(next) => {
            let current: Option<Address> = env.storage().instance().get(&DataKey::SkimRecipient);
            if current.as_ref() == Some(next) {
                return Err(GovernanceError::NoChange);
            }
            Ok(TimelockDecision::Timelocked)
        }
        GovernanceAction::Skim(_) => Ok(TimelockDecision::Timelocked),
        GovernanceAction::SetCurator(_)
        | GovernanceAction::SetGovernance(_)
        | GovernanceAction::SetSupplyQueue(_, _)
        | GovernanceAction::SetAllocators(_)
        | GovernanceAction::SetAllowedAdapters(_)
        | GovernanceAction::Upgrade(_)
        | GovernanceAction::Migrate
        | GovernanceAction::CancelMigration => Ok(TimelockDecision::Timelocked),
        GovernanceAction::Other(key, payload_hash) => {
            let approved: bool = env
                .storage()
                .instance()
                .get(&DataKey::ApprovedOther(key.clone(), payload_hash.clone()))
                .unwrap_or(false);
            if approved {
                return Err(GovernanceError::NoChange);
            }
            Ok(TimelockDecision::Timelocked)
        }
    }
}

fn to_wad(value: i128) -> Result<Wad, GovernanceError> {
    nonnegative_i128_to_u128(value)
        .map(Wad::from)
        .ok_or(GovernanceError::InvalidInput)
}

fn to_optional_wad(value: Option<i128>) -> Result<Option<Wad>, GovernanceError> {
    match value {
        Some(v) => Ok(Some(to_wad(v)?)),
        None => Ok(None),
    }
}

fn validate_timelock_ns(value: u64) -> Result<(), GovernanceError> {
    if !(MIN_TIMELOCK_NS..=MAX_TIMELOCK_NS).contains(&value) {
        return Err(GovernanceError::TimelockOutOfBounds);
    }
    Ok(())
}

fn load_timelocks(env: &Env) -> Timelocks {
    if let Some(timelocks) = env.storage().instance().get(&DataKey::Timelocks) {
        return timelocks;
    }

    let default_ns = env
        .storage()
        .instance()
        .get(&DataKey::TimelockNs)
        .unwrap_or(0);
    Timelocks::from_default(default_ns)
}

fn next_proposal_id(env: &Env) -> Result<u64, GovernanceError> {
    let current: u64 = env
        .storage()
        .instance()
        .get(&DataKey::NextProposalId)
        .unwrap_or(1);
    let next = current
        .checked_add(1)
        .ok_or(GovernanceError::ArithmeticOverflow)?;
    env.storage()
        .instance()
        .set(&DataKey::NextProposalId, &next);
    Ok(current)
}

fn pending_page_id(proposal_id: u64) -> u64 {
    proposal_id / PENDING_PAGE_SIZE
}

fn load_pending_page_index(env: &Env) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::PendingPageIndex)
        .unwrap_or_else(|| Vec::new(env))
}

fn save_pending_page_index(env: &Env, pages: &Vec<u64>) {
    env.storage()
        .persistent()
        .set(&DataKey::PendingPageIndex, pages);
}

fn load_pending_page(env: &Env, page: u64) -> Vec<StoredPending> {
    env.storage()
        .persistent()
        .get(&DataKey::PendingPage(page))
        .unwrap_or_else(|| Vec::new(env))
}

fn save_pending_page(env: &Env, page: u64, entries: &Vec<StoredPending>) {
    if entries.is_empty() {
        env.storage()
            .persistent()
            .remove(&DataKey::PendingPage(page));
    } else {
        env.storage()
            .persistent()
            .set(&DataKey::PendingPage(page), entries);
    }
}

fn push_unique_page(pages: &mut Vec<u64>, page: u64) {
    if !pages.iter().any(|existing| existing == page) {
        pages.push_back(page);
    }
}

fn extend_pending_queue_ttl(env: &Env) {
    let storage = env.storage().persistent();
    if storage.has(&DataKey::PendingPageIndex) {
        storage.extend_ttl(
            &DataKey::PendingPageIndex,
            INSTANCE_TTL_THRESHOLD,
            INSTANCE_TTL_EXTEND_TO,
        );
    }
    for page in load_pending_page_index(env).iter() {
        let key = DataKey::PendingPage(page);
        if storage.has(&key) {
            storage.extend_ttl(&key, INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
        }
    }
}

fn load_queue(env: &Env) -> PendingActions<QueuedProposal> {
    let mut entries = alloc::vec::Vec::new();
    let pages = load_pending_page_index(env);

    for page in pages.iter() {
        for item in load_pending_page(env, page).iter() {
            if pending_page_id(item.id) == page {
                entries.push(PendingValue {
                    value: QueuedProposal {
                        id: item.id,
                        action: item.action.clone(),
                    },
                    ready_at_ns: TimestampNs(item.valid_at_ns),
                });
            }
        }
    }

    PendingActions::from_restored_entries(entries)
}

fn save_queue(env: &Env, queue: &PendingActions<QueuedProposal>) {
    let old_pages = load_pending_page_index(env);
    let mut new_pages = Vec::new(env);

    for entry in queue.iter() {
        push_unique_page(&mut new_pages, pending_page_id(entry.value.id));
    }

    for page in new_pages.iter() {
        let mut stored = Vec::new(env);
        for entry in queue.iter() {
            if pending_page_id(entry.value.id) == page {
                stored.push_back(StoredPending {
                    id: entry.value.id,
                    action: entry.value.action.clone(),
                    valid_at_ns: entry.ready_at_ns.into(),
                });
            }
        }
        save_pending_page(env, page, &stored);
    }

    for page in old_pages.iter() {
        if !new_pages.iter().any(|existing| existing == page) {
            env.storage()
                .persistent()
                .remove(&DataKey::PendingPage(page));
        }
    }

    save_pending_page_index(env, &new_pages);
    extend_pending_queue_ttl(env);
}

fn load_pending_ids(env: &Env) -> Vec<u64> {
    let queue = load_queue(env);
    let mut ids = Vec::new(env);
    for entry in queue.iter() {
        ids.push_back(entry.value.id);
    }
    ids
}

fn load_proposal(env: &Env, proposal_id: u64) -> Result<PendingProposal, GovernanceError> {
    let queue = load_queue(env);
    for entry in queue.iter() {
        if entry.value.id == proposal_id {
            return Ok(PendingProposal {
                id: entry.value.id,
                action: entry.value.action.clone(),
                valid_after_ns: entry.ready_at_ns.into(),
            });
        }
    }
    Err(GovernanceError::ProposalNotFound)
}

fn queued_proposal_key_by_id(proposal: &QueuedProposal) -> ProposalKey {
    ProposalKey::ProposalId(proposal.id)
}

fn queued_proposal_kind(proposal: &QueuedProposal) -> GovernanceActionKind {
    action_kind(&proposal.action)
}

fn revoke_by_action_key(env: &Env, key: &GovernanceActionKey) -> u32 {
    let mut queue = load_queue(env);
    let mut revoked_ids = Vec::new(env);

    for removed in queue
        .revoke_by_key(key, QueuedProposal::action_key)
        .into_iter()
    {
        revoked_ids.push_back(removed.id);
    }

    if revoked_ids.is_empty() {
        return 0;
    }

    save_queue(env, &queue);

    for id in revoked_ids.iter() {
        ProposalRevoked { id }.publish(env);
    }

    revoked_ids.len()
}

#[allow(clippy::too_many_lines)]
fn execute_action(env: &Env, action: &GovernanceAction) -> Result<(), GovernanceError> {
    let vault = get_address(env, DataKey::Vault)?;

    match action {
        GovernanceAction::SetAdmin(new_admin) => {
            env.storage().instance().set(&DataKey::Admin, new_admin);
        }
        GovernanceAction::SetPaused(paused) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage()
                .instance()
                .set(&DataKey::CurrentPaused, paused);
        }
        GovernanceAction::SetCurator(_)
        | GovernanceAction::SetGovernance(_)
        | GovernanceAction::SetSupplyQueue(_, _)
        | GovernanceAction::SetAllocators(_)
        | GovernanceAction::SetAllowedAdapters(_) => {
            execute_vault_governance_action(env, &vault, action)?
        }
        GovernanceAction::SetFees(params) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage().instance().set(&DataKey::CurrentFees, params);
        }
        GovernanceAction::SetWithdrawalCooldown(withdrawal_cooldown_ns) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage().instance().set(
                &DataKey::CurrentWithdrawalCooldownNs,
                withdrawal_cooldown_ns,
            );
        }
        GovernanceAction::SetIdleResyncCooldown(idle_resync_cooldown_ns) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage().instance().set(
                &DataKey::CurrentIdleResyncCooldownNs,
                idle_resync_cooldown_ns,
            );
        }
        GovernanceAction::SetRestrictions(mode, accounts) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage()
                .instance()
                .set(&DataKey::CurrentRestrictionMode, mode);
            env.storage()
                .instance()
                .set(&DataKey::CurrentRestrictionAccounts, accounts);
        }
        GovernanceAction::SetSentinel(sentinel) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage().instance().set(&DataKey::Sentinel, sentinel);
        }
        GovernanceAction::SetCap(market_id, cap) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage()
                .instance()
                .set(&DataKey::CurrentCap(*market_id), cap);
        }
        GovernanceAction::RemoveMarket(market_id) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage()
                .instance()
                .remove(&DataKey::CurrentCap(*market_id));
            env.storage()
                .instance()
                .remove(&DataKey::CurrentCapGroupMembership(*market_id));
        }
        GovernanceAction::SetGroupCap(cap_group_id, cap) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage()
                .instance()
                .set(&DataKey::CurrentCapGroupCap(cap_group_id.clone()), cap);
            env.storage()
                .instance()
                .set(&DataKey::KnownCapGroupCap(cap_group_id.clone()), &true);
        }
        GovernanceAction::SetGroupRelCap(cap_group_id, relative_cap) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage().instance().set(
                &DataKey::CurrentCapGroupRelCap(cap_group_id.clone()),
                relative_cap,
            );
            env.storage()
                .instance()
                .set(&DataKey::KnownCapGroupRelCap(cap_group_id.clone()), &true);
        }
        GovernanceAction::SetGroupMember(market_id, cap_group_id) => {
            execute_vault_governance_action(env, &vault, action)?;
            let key = DataKey::CurrentCapGroupMembership(*market_id);
            if cap_group_id.is_empty() {
                env.storage().instance().remove(&key);
            } else {
                env.storage().instance().set(&key, cap_group_id);
            }
            env.storage()
                .instance()
                .set(&DataKey::KnownCapGroupMembership(*market_id), &true);
        }
        GovernanceAction::SetSkimRecipient(recipient) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage()
                .instance()
                .set(&DataKey::SkimRecipient, recipient);
        }
        GovernanceAction::Skim(_) => execute_vault_governance_action(env, &vault, action)?,
        GovernanceAction::Upgrade(new_wasm_hash) => {
            let governance = env.current_contract_address();
            authorize_and_invoke(
                env,
                &vault,
                Symbol::new(env, "upgrade"),
                Vec::from_array(
                    env,
                    [
                        new_wasm_hash.clone().into_val(env),
                        governance.into_val(env),
                    ],
                ),
            );
        }
        GovernanceAction::Migrate => {
            let governance = env.current_contract_address();
            authorize_and_invoke(
                env,
                &vault,
                Symbol::new(env, "migrate"),
                Vec::from_array(env, [governance.into_val(env)]),
            );
        }
        GovernanceAction::CancelMigration => {
            let governance = env.current_contract_address();
            let command = VaultCommand::CancelMigration {
                caller: sdk_address_to_alloc_string(&governance)?,
            };
            execute_vault_command(env, &vault, &command);
        }
        GovernanceAction::SetTimelock(kind, new_timelock_ns) => {
            validate_timelock_ns(*new_timelock_ns)?;
            let mut timelocks = load_timelocks(env);
            timelocks.set(*kind, *new_timelock_ns);
            env.storage()
                .instance()
                .set(&DataKey::Timelocks, &timelocks);
            env.storage()
                .instance()
                .set(&DataKey::TimelockNs, &timelocks.timelock_config_ns);
        }
        GovernanceAction::Other(key, payload_hash) => {
            env.storage().instance().set(
                &DataKey::ApprovedOther(key.clone(), payload_hash.clone()),
                &true,
            );
        }
    }

    Ok(())
}

fn execute_vault_governance_action(
    env: &Env,
    vault: &Address,
    action: &GovernanceAction,
) -> Result<(), GovernanceError> {
    let payload =
        governance_payload_for_action(env, action)?.ok_or(GovernanceError::InvalidInput)?;
    let governance = env.current_contract_address();
    authorize_and_invoke(
        env,
        vault,
        Symbol::new(env, "execute_governance"),
        Vec::from_array(env, [governance.into_val(env), payload.into_val(env)]),
    );
    Ok(())
}

fn execute_vault_governance_action_as_caller(
    env: &Env,
    vault: &Address,
    caller: &Address,
    action: &GovernanceAction,
) -> Result<(), GovernanceError> {
    let payload =
        governance_payload_for_action(env, action)?.ok_or(GovernanceError::InvalidInput)?;
    env.invoke_contract::<()>(
        vault,
        &Symbol::new(env, "execute_governance"),
        Vec::from_array(env, [caller.clone().into_val(env), payload.into_val(env)]),
    );
    Ok(())
}

fn governance_payload_for_action(
    env: &Env,
    action: &GovernanceAction,
) -> Result<Option<Bytes>, GovernanceError> {
    let command = match action {
        GovernanceAction::SetCurator(curator) => Some(GovernanceCommand::SetGovernanceConfig {
            kind: GOVERNANCE_CONFIG_KIND_CURATOR,
            primary: Some(sdk_address_to_alloc_string(curator)?),
            many: None,
            value_a: None,
            value_b: None,
        }),
        GovernanceAction::SetGovernance(governance) => {
            Some(GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_GOVERNANCE,
                primary: Some(sdk_address_to_alloc_string(governance)?),
                many: None,
                value_a: None,
                value_b: None,
            })
        }
        GovernanceAction::SetSentinel(sentinel) => Some(GovernanceCommand::SetGovernanceConfig {
            kind: GOVERNANCE_CONFIG_KIND_SENTINEL,
            primary: Some(sdk_address_to_alloc_string(sentinel)?),
            many: None,
            value_a: None,
            value_b: None,
        }),
        GovernanceAction::SetAllocators(allocators) => {
            Some(GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_ALLOCATORS,
                primary: None,
                many: Some(soroban_address_vec_to_alloc(allocators)?),
                value_a: None,
                value_b: None,
            })
        }
        GovernanceAction::SetAllowedAdapters(adapters) => {
            Some(GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS,
                primary: None,
                many: Some(soroban_address_vec_to_alloc(adapters)?),
                value_a: None,
                value_b: None,
            })
        }
        GovernanceAction::SetSkimRecipient(recipient) => {
            Some(GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_SKIM_RECIPIENT,
                primary: Some(sdk_address_to_alloc_string(recipient)?),
                many: None,
                value_a: None,
                value_b: None,
            })
        }
        GovernanceAction::SetWithdrawalCooldown(withdrawal_cooldown_ns) => {
            Some(GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_WITHDRAWAL_COOLDOWN,
                primary: None,
                many: None,
                value_a: Some(i128::from(*withdrawal_cooldown_ns)),
                value_b: None,
            })
        }
        GovernanceAction::SetIdleResyncCooldown(idle_resync_cooldown_ns) => {
            Some(GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_IDLE_RESYNC_COOLDOWN,
                primary: None,
                many: None,
                value_a: Some(i128::from(*idle_resync_cooldown_ns)),
                value_b: None,
            })
        }
        GovernanceAction::Skim(token) => Some(GovernanceCommand::Skim {
            token: sdk_address_to_alloc_string(token)?,
        }),
        GovernanceAction::SetSupplyQueue(target_ids, adapters) => {
            Some(GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                target_ids: Some(soroban_u32_vec_to_alloc(target_ids)),
                mode: None,
                accounts: if adapters.is_empty() {
                    None
                } else {
                    Some(soroban_address_vec_to_alloc(adapters)?)
                },
                market_id: None,
                cap_group_id: None,
                value: None,
                value_b: None,
                value_c: None,
            })
        }
        GovernanceAction::SetPaused(paused) => Some(GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_PAUSED,
            target_ids: None,
            mode: Some(u32::from(*paused)),
            accounts: None,
            market_id: None,
            cap_group_id: None,
            value: None,
            value_b: None,
            value_c: None,
        }),
        GovernanceAction::SetFees(params) => Some(GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_FEES,
            target_ids: None,
            mode: None,
            accounts: Some(alloc::vec![
                sdk_address_to_alloc_string(&params.performance_recipient)?,
                sdk_address_to_alloc_string(&params.management_recipient)?,
            ]),
            market_id: None,
            cap_group_id: None,
            value: Some(params.performance_fee_wad),
            value_b: Some(params.management_fee_wad),
            value_c: params.max_growth_rate_wad,
        }),
        GovernanceAction::SetRestrictions(mode, accounts) => {
            Some(GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_RESTRICTIONS,
                target_ids: None,
                mode: Some(mode.as_u32()),
                accounts: Some(soroban_address_vec_to_alloc(accounts)?),
                market_id: None,
                cap_group_id: None,
                value: None,
                value_b: None,
                value_c: None,
            })
        }
        GovernanceAction::SetCap(market_id, cap) => Some(GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_CAP,
            target_ids: None,
            mode: None,
            accounts: None,
            market_id: Some(*market_id),
            cap_group_id: None,
            value: Some(*cap),
            value_b: None,
            value_c: None,
        }),
        GovernanceAction::RemoveMarket(market_id) => Some(GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_REMOVE_MARKET,
            target_ids: None,
            mode: None,
            accounts: None,
            market_id: Some(*market_id),
            cap_group_id: None,
            value: None,
            value_b: None,
            value_c: None,
        }),
        GovernanceAction::SetGroupCap(cap_group_id, cap) => {
            Some(GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_GROUP,
                target_ids: None,
                mode: Some(0),
                accounts: None,
                market_id: None,
                cap_group_id: Some(sdk_string_to_alloc_string(cap_group_id)?),
                value: Some(*cap),
                value_b: None,
                value_c: None,
            })
        }
        GovernanceAction::SetGroupRelCap(cap_group_id, relative_cap_wad) => {
            Some(GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_GROUP,
                target_ids: None,
                mode: Some(1),
                accounts: None,
                market_id: None,
                cap_group_id: Some(sdk_string_to_alloc_string(cap_group_id)?),
                value: Some(*relative_cap_wad),
                value_b: None,
                value_c: None,
            })
        }
        GovernanceAction::SetGroupMember(market_id, cap_group_id) => {
            let cap_group_id = if cap_group_id.is_empty() {
                None
            } else {
                Some(sdk_string_to_alloc_string(cap_group_id)?)
            };
            Some(GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_GROUP,
                target_ids: None,
                mode: Some(2),
                accounts: None,
                market_id: Some(*market_id),
                cap_group_id,
                value: None,
                value_b: None,
                value_c: None,
            })
        }
        GovernanceAction::Upgrade(_)
        | GovernanceAction::Migrate
        | GovernanceAction::CancelMigration
        | GovernanceAction::SetAdmin(_)
        | GovernanceAction::SetTimelock(_, _)
        | GovernanceAction::Other(_, _) => None,
    };

    Ok(command.map(|command| Bytes::from_slice(env, &command.encode())))
}

fn sdk_address_to_alloc_string(address: &Address) -> Result<AllocString, GovernanceError> {
    let raw = address.to_string().to_bytes().to_alloc_vec();
    AllocString::from_utf8(raw).map_err(|_| GovernanceError::InvalidInput)
}

fn sdk_string_to_alloc_string(value: &String) -> Result<AllocString, GovernanceError> {
    AllocString::from_utf8(value.to_bytes().to_alloc_vec())
        .map_err(|_| GovernanceError::InvalidInput)
}

fn soroban_u32_vec_to_alloc(values: &Vec<u32>) -> alloc::vec::Vec<u32> {
    let mut result = alloc::vec::Vec::new();
    for value in values.iter() {
        result.push(value);
    }
    result
}

fn soroban_address_vec_to_alloc(
    values: &Vec<Address>,
) -> Result<alloc::vec::Vec<AllocString>, GovernanceError> {
    let mut result = alloc::vec::Vec::new();
    for value in values.iter() {
        result.push(sdk_address_to_alloc_string(&value)?);
    }
    Ok(result)
}

fn execute_vault_command(env: &Env, vault: &Address, command: &VaultCommand) {
    let payload = Bytes::from_slice(env, &command.encode());
    authorize_and_invoke_bytes(env, vault, Symbol::new(env, "execute"), payload);
}

fn authorize_and_invoke(env: &Env, vault: &Address, fn_name: Symbol, args: Vec<soroban_sdk::Val>) {
    let args_for_auth = args.clone();

    env.authorize_as_current_contract(Vec::from_array(
        env,
        [InvokerContractAuthEntry::Contract(SubContractInvocation {
            context: ContractContext {
                contract: vault.clone(),
                fn_name: fn_name.clone(),
                args: args_for_auth,
            },
            sub_invocations: Vec::new(env),
        })],
    ));

    env.invoke_contract::<()>(vault, &fn_name, args);
}

fn authorize_and_invoke_bytes(env: &Env, vault: &Address, fn_name: Symbol, payload: Bytes) {
    let args = Vec::from_array(env, [payload.into_val(env)]);
    let args_for_auth = args.clone();

    env.authorize_as_current_contract(Vec::from_array(
        env,
        [InvokerContractAuthEntry::Contract(SubContractInvocation {
            context: ContractContext {
                contract: vault.clone(),
                fn_name: fn_name.clone(),
                args: args_for_auth,
            },
            sub_invocations: Vec::new(env),
        })],
    ));

    let _result = env.invoke_contract::<Bytes>(vault, &fn_name, args);
}

fn get_address(env: &Env, key: DataKey) -> Result<Address, GovernanceError> {
    env.storage()
        .instance()
        .get(&key)
        .ok_or(GovernanceError::MissingConfig)
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), GovernanceError> {
    caller.require_auth();
    let admin = get_address(env, DataKey::Admin)?;
    if caller != &admin {
        return Err(GovernanceError::Unauthorized);
    }
    Ok(())
}

fn revoker_role(env: &Env, caller: &Address) -> Result<RevokerRole, GovernanceError> {
    caller.require_auth();
    let admin = get_address(env, DataKey::Admin)?;
    if caller == &admin {
        return Ok(RevokerRole::Admin);
    }
    let sentinel: Option<Address> = env.storage().instance().get(&DataKey::Sentinel);
    if sentinel.as_ref() == Some(caller) {
        return Ok(RevokerRole::Sentinel);
    }
    Err(GovernanceError::Unauthorized)
}

fn require_sentinel(env: &Env, caller: &Address) -> Result<(), GovernanceError> {
    caller.require_auth();
    let sentinel: Option<Address> = env.storage().instance().get(&DataKey::Sentinel);
    if sentinel.as_ref() == Some(caller) {
        return Ok(());
    }
    Err(GovernanceError::Unauthorized)
}

fn require_revoke_kind(
    env: &Env,
    caller: &Address,
    kind: GovernanceActionKind,
) -> Result<(), GovernanceError> {
    let role = revoker_role(env, caller)?;
    if can_revoke_kind(role, kind) {
        Ok(())
    } else {
        Err(GovernanceError::Unauthorized)
    }
}

fn can_revoke_kind(role: RevokerRole, kind: GovernanceActionKind) -> bool {
    match role {
        RevokerRole::Admin => true,
        // Sentinel is the active operational-risk backstop. It may cancel
        // timelocked operational/economic proposals that could affect allocation,
        // accounting, restrictions, or emergency controls. Ownership/control-plane
        // transfers, curator replacement, skim actions, and arbitrary `Other`
        // approvals remain admin-only so the sentinel cannot block governance
        // handoff or unrelated external approvals.
        RevokerRole::Sentinel => matches!(
            kind,
            GovernanceActionKind::Pause
                | GovernanceActionKind::Sentinel
                | GovernanceActionKind::SupplyQueue
                | GovernanceActionKind::Allocators
                | GovernanceActionKind::AllowedAdapters
                | GovernanceActionKind::Fees
                | GovernanceActionKind::WithdrawalCooldown
                | GovernanceActionKind::IdleResyncCooldown
                | GovernanceActionKind::Restrictions
                | GovernanceActionKind::TimelockConfig
                | GovernanceActionKind::Cap
                | GovernanceActionKind::MarketRemoval
                | GovernanceActionKind::CapGroup
        ),
    }
}

fn require_vault_caller(env: &Env, caller: &Address) -> Result<(), GovernanceError> {
    caller.require_auth();
    let vault = get_address(env, DataKey::Vault)?;
    if caller != &vault {
        return Err(GovernanceError::Unauthorized);
    }
    Ok(())
}

fn require_not_abdicated(env: &Env, action: &GovernanceAction) -> Result<(), GovernanceError> {
    let kind = action_kind(action);
    let abdicated: bool = env
        .storage()
        .instance()
        .get(&DataKey::Abdicated(kind))
        .unwrap_or(false);
    if abdicated {
        return Err(GovernanceError::Abdicated);
    }
    Ok(())
}

fn restriction_snapshot(
    mode: RestrictionMode,
    accounts: &Vec<Address>,
) -> Option<GovernanceRestrictions<Address>> {
    let members: AllocVec<Address> = accounts.iter().collect();
    match mode {
        RestrictionMode::None => None,
        RestrictionMode::Blacklist => Some(GovernanceRestrictions::blacklist(members)),
        RestrictionMode::Whitelist => Some(GovernanceRestrictions::whitelist(members)),
    }
}

fn restrictions_change_is_relaxed(
    env: &Env,
    next_mode: RestrictionMode,
    next_accounts: &Vec<Address>,
) -> bool {
    let current_mode: RestrictionMode = env
        .storage()
        .instance()
        .get(&DataKey::CurrentRestrictionMode)
        .unwrap_or(RestrictionMode::None);
    let current_accounts: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::CurrentRestrictionAccounts)
        .unwrap_or_else(|| Vec::new(env));
    GovernanceRestrictions::determine_relaxed(
        &restriction_snapshot(current_mode, &current_accounts),
        &restriction_snapshot(next_mode, next_accounts),
    )
}

fn ledger_timestamp_ns(env: &Env) -> Result<TimestampNs, GovernanceError> {
    seconds_to_nanoseconds(env.ledger().timestamp())
        .map(TimestampNs)
        .ok_or(GovernanceError::ArithmeticOverflow)
}

fn is_contract_address(addr: &Address) -> bool {
    matches!(
        addr.executable(),
        Some(Executable::Wasm(_)) | Some(Executable::StellarAsset)
    )
}

fn require_contract_address(addr: &Address) -> Result<(), GovernanceError> {
    if is_contract_address(addr) {
        Ok(())
    } else {
        Err(GovernanceError::InvalidInput)
    }
}

fn require_wasm_contract_address(addr: &Address) -> Result<(), GovernanceError> {
    match addr.executable() {
        Some(Executable::Wasm(_)) => Ok(()),
        _ => Err(GovernanceError::InvalidInput),
    }
}

fn require_governance_target(env: &Env, governance: &Address) -> Result<(), GovernanceError> {
    require_wasm_contract_address(governance)?;
    let vault = get_address(env, DataKey::Vault)?;
    if governance == &vault || governance == &env.current_contract_address() {
        return Err(GovernanceError::InvalidInput);
    }
    Ok(())
}

fn require_constructor_topology(
    env: &Env,
    admin: &Address,
    vault: &Address,
) -> Result<(), GovernanceError> {
    let current = env.current_contract_address();
    if admin == vault || admin == &current || vault == &current {
        return Err(GovernanceError::InvalidInput);
    }
    Ok(())
}

fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

#[cfg(test)]
mod tests;
