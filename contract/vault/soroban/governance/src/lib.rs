#![no_std]

extern crate alloc;

mod types;
pub use types::*;

use alloc::string::String as AllocString;
use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    Address, Bytes, BytesN, Env, IntoVal, String as SdkString, Symbol, Vec,
};
#[cfg(feature = "contract")]
use soroban_sdk::{contract, contractimpl};
use templar_curator_primitives::governance::{
    timelock_config_decision, CapChangeError, FeeChangeError, FeeConfig, MembershipChangeError,
    PendingActions, PendingValue, RelativeCapChangeError, Restrictions as SharedRestrictions,
    TakePending, TimelockConfigError, TimelockDecision,
};
use templar_curator_primitives::{nonnegative_i128_to_u128, seconds_to_nanoseconds};
use templar_soroban_shared_types::{
    GovernanceCommand, GOVERNANCE_CONFIG_KIND_ALLOCATORS, GOVERNANCE_CONFIG_KIND_CURATOR,
    GOVERNANCE_CONFIG_KIND_GOVERNANCE, GOVERNANCE_CONFIG_KIND_GUARDIANS,
    GOVERNANCE_CONFIG_KIND_SENTINEL, GOVERNANCE_CONFIG_KIND_SKIM_RECIPIENT,
    GOVERNANCE_POLICY_KIND_CAP, GOVERNANCE_POLICY_KIND_FEES, GOVERNANCE_POLICY_KIND_GROUP,
    GOVERNANCE_POLICY_KIND_PAUSED, GOVERNANCE_POLICY_KIND_REMOVE_MARKET,
    GOVERNANCE_POLICY_KIND_RESTRICTIONS, GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
};
use templar_vault_kernel::math::wad::Wad;
use templar_vault_kernel::{DurationNs, TimestampNs};

const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
const INSTANCE_TTL_EXTEND_TO: u32 = 3_110_400;
const MIN_TIMELOCK_NS: u64 = 0;
const MAX_TIMELOCK_NS: u64 = u64::MAX;

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
enum ProposalKey {
    ProposalId(u64),
    #[allow(dead_code)]
    Action(GovernanceActionKey),
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
enum GovernanceActionKey {
    Pause,
    Curator,
    Governance,
    SupplyQueue,
    Fees,
    Restrictions,
    Guardian,
    Sentinel,
    Cap(u32),
    MarketRemoval(u32),
    CapGroupCap(SdkString),
    CapGroupRelativeCap(SdkString),
    CapGroupMembership(u32),
    SkimRecipient,
    Skim(Address),
    Allocator,
    TimelockConfig(TimelockScope),
    Other(Symbol, BytesN<32>),
}

impl GovernanceAction {
    fn pending_key(&self) -> GovernanceActionKey {
        match self {
            Self::SetPaused(_) => GovernanceActionKey::Pause,
            Self::SetCurator(_) => GovernanceActionKey::Curator,
            Self::SetGovernance(_) => GovernanceActionKey::Governance,
            Self::SetSupplyQueue(_) => GovernanceActionKey::SupplyQueue,
            Self::SetFees(_) => GovernanceActionKey::Fees,
            Self::SetRestrictions(_, _) => GovernanceActionKey::Restrictions,
            Self::SetGuardian(_) => GovernanceActionKey::Guardian,
            Self::SetSentinel(_) => GovernanceActionKey::Sentinel,
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
            Self::SetAllocators(_) => GovernanceActionKey::Allocator,
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

#[cfg_attr(feature = "contract", contract)]
pub struct SorobanVaultGovernanceContract;

#[cfg_attr(feature = "contract", contractimpl)]
impl SorobanVaultGovernanceContract {
    pub fn __constructor(
        env: Env,
        admin: Address,
        vault: Address,
        timelock_ns: u64,
    ) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        require_contract_address(&vault)?;
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
            .set(&DataKey::PendingQueue, &Vec::<StoredPending>::new(&env));
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
        env.storage()
            .instance()
            .set(&DataKey::CurrentRestrictionMode, &RestrictionMode::None);
        env.storage().instance().set(
            &DataKey::CurrentRestrictionAccounts,
            &Vec::<Address>::new(&env),
        );
        env.storage()
            .instance()
            .set(&DataKey::CurrentAllocators, &Vec::<Address>::new(&env));
        Ok(())
    }

    pub fn submit_set_paused(
        env: Env,
        caller: Address,
        paused: bool,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetPaused(paused))
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
        require_contract_address(&governance)?;
        Self::submit(env, caller, GovernanceAction::SetGovernance(governance))
    }

    pub fn submit_set_supply_queue(
        env: Env,
        caller: Address,
        target_ids: Vec<u32>,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetSupplyQueue(target_ids))
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

    pub fn set_fees(env: Env, caller: Address, fees: Fees) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetFees(fees))
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

    pub fn set_restrictions(
        env: Env,
        caller: Address,
        restrictions: Restrictions,
    ) -> Result<u64, GovernanceError> {
        let (mode, accounts) = restrictions.into_parts(&env);
        Self::submit(
            env,
            caller,
            GovernanceAction::SetRestrictions(mode, accounts),
        )
    }

    pub fn submit_set_guardian(
        env: Env,
        caller: Address,
        guardian: Address,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetGuardian(guardian))
    }

    pub fn submit_set_sentinel(
        env: Env,
        caller: Address,
        sentinel: Address,
    ) -> Result<u64, GovernanceError> {
        Self::submit(env, caller, GovernanceAction::SetSentinel(sentinel))
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
            GovernanceAction::SetTimelock(TimelockScope::One(kind), new_timelock_ns),
        )
    }

    pub fn submit_timelock(
        env: Env,
        caller: Address,
        new_timelock_ns: u64,
        kind: Option<TimelockKind>,
    ) -> Result<u64, GovernanceError> {
        let scope = timelock_scope(kind);
        Self::submit(
            env,
            caller,
            GovernanceAction::SetTimelock(scope, new_timelock_ns),
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
        cap_group_id: SdkString,
        new_cap: i128,
    ) -> Result<u64, GovernanceError> {
        Self::submit(
            env,
            caller,
            GovernanceAction::SetGroupCap(cap_group_id, Some(new_cap)),
        )
    }

    pub fn submit_set_group_rel_cap(
        env: Env,
        caller: Address,
        cap_group_id: SdkString,
        new_relative_cap_wad: i128,
    ) -> Result<u64, GovernanceError> {
        Self::submit(
            env,
            caller,
            GovernanceAction::SetGroupRelCap(cap_group_id, Some(new_relative_cap_wad)),
        )
    }

    pub fn submit_set_group_member(
        env: Env,
        caller: Address,
        market_id: u32,
        cap_group_id: SdkString,
    ) -> Result<u64, GovernanceError> {
        let cap_group_id = if cap_group_id.is_empty() {
            None
        } else {
            Some(cap_group_id)
        };
        Self::submit(
            env,
            caller,
            GovernanceAction::SetGroupMember(market_id, cap_group_id),
        )
    }

    pub fn submit_cap_group_update(
        env: Env,
        caller: Address,
        update: CapGroupUpdate,
    ) -> Result<u64, GovernanceError> {
        let action = match update {
            CapGroupUpdate::SetCap(cap_group_id, value) => {
                GovernanceAction::SetGroupCap(cap_group_id, Some(value))
            }
            CapGroupUpdate::RemoveCap(cap_group_id) => {
                GovernanceAction::SetGroupCap(cap_group_id, None)
            }
            CapGroupUpdate::SetRelativeCap(cap_group_id, value) => {
                GovernanceAction::SetGroupRelCap(cap_group_id, Some(value))
            }
            CapGroupUpdate::RemoveRelativeCap(cap_group_id) => {
                GovernanceAction::SetGroupRelCap(cap_group_id, None)
            }
            CapGroupUpdate::SetMember(market_id, cap_group_id) => {
                GovernanceAction::SetGroupMember(market_id, Some(cap_group_id))
            }
            CapGroupUpdate::RemoveMember(market_id) => {
                GovernanceAction::SetGroupMember(market_id, None)
            }
        };
        Self::submit(env, caller, action)
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

    pub fn set_allocators(
        env: Env,
        caller: Address,
        allocators: Vec<Address>,
    ) -> Result<u64, GovernanceError> {
        let allocators = dedup_addresses(&env, &allocators);
        Self::submit(env, caller, GovernanceAction::SetAllocators(allocators))
    }

    pub fn set_is_allocator(
        env: Env,
        caller: Address,
        account: Address,
        allowed: bool,
    ) -> Result<u64, GovernanceError> {
        let mut allocators: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::CurrentAllocators)
            .unwrap_or_else(|| Vec::new(&env));
        let contains = allocators.iter().any(|existing| existing == account);

        if allowed && !contains {
            allocators.push_back(account);
        } else if !allowed && contains {
            let mut next = Vec::new(&env);
            for allocator in allocators.iter() {
                if allocator != account {
                    next.push_back(allocator);
                }
            }
            allocators = next;
        }

        let allocators = dedup_addresses(&env, &allocators);
        Self::submit(env, caller, GovernanceAction::SetAllocators(allocators))
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
        require_revoker(&env, &caller)?;
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

    pub fn accept_fees(env: Env, caller: Address) -> Result<u64, GovernanceError> {
        accept_by_action_key(&env, &caller, GovernanceActionKey::Fees)
    }

    pub fn accept_cap(env: Env, caller: Address, market_id: u32) -> Result<u64, GovernanceError> {
        accept_by_action_key(&env, &caller, GovernanceActionKey::Cap(market_id))
    }

    pub fn accept_market_removal(
        env: Env,
        caller: Address,
        market_id: u32,
    ) -> Result<u64, GovernanceError> {
        accept_by_action_key(&env, &caller, GovernanceActionKey::MarketRemoval(market_id))
    }

    pub fn accept_cap_group_update(
        env: Env,
        caller: Address,
        key: CapGroupUpdateKey,
    ) -> Result<u64, GovernanceError> {
        accept_by_action_key(&env, &caller, cap_group_key_to_action_key(key))
    }

    pub fn accept_timelock(
        env: Env,
        caller: Address,
        kind: Option<TimelockKind>,
    ) -> Result<u64, GovernanceError> {
        accept_by_action_key(
            &env,
            &caller,
            GovernanceActionKey::TimelockConfig(timelock_scope(kind)),
        )
    }

    pub fn accept_allocators(env: Env, caller: Address) -> Result<u64, GovernanceError> {
        accept_by_action_key(&env, &caller, GovernanceActionKey::Allocator)
    }

    pub fn revoke(env: Env, caller: Address, proposal_id: u64) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        require_revoker(&env, &caller)?;
        let mut queue = load_queue(&env);
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
        require_revoker(&env, &caller)?;
        let removed = revoke_where(&env, |action| action_kind(action) == kind);
        if removed == 0 {
            return Err(GovernanceError::ProposalNotFound);
        }
        Ok(removed)
    }

    pub fn revoke_pending_fees(env: Env, caller: Address) -> Result<u32, GovernanceError> {
        revoke_by_public_action_key(&env, &caller, GovernanceActionKey::Fees)
    }

    pub fn revoke_pending_cap(
        env: Env,
        caller: Address,
        market_id: u32,
    ) -> Result<u32, GovernanceError> {
        revoke_by_public_action_key(&env, &caller, GovernanceActionKey::Cap(market_id))
    }

    pub fn revoke_pending_market_removal(
        env: Env,
        caller: Address,
        market_id: u32,
    ) -> Result<u32, GovernanceError> {
        revoke_by_public_action_key(&env, &caller, GovernanceActionKey::MarketRemoval(market_id))
    }

    pub fn revoke_pending_cap_group_update(
        env: Env,
        caller: Address,
        key: CapGroupUpdateKey,
    ) -> Result<u32, GovernanceError> {
        revoke_by_public_action_key(&env, &caller, cap_group_key_to_action_key(key))
    }

    pub fn revoke_pending_timelock(
        env: Env,
        caller: Address,
        kind: Option<TimelockKind>,
    ) -> Result<u32, GovernanceError> {
        revoke_by_public_action_key(
            &env,
            &caller,
            GovernanceActionKey::TimelockConfig(timelock_scope(kind)),
        )
    }

    pub fn revoke_pending_allocators(env: Env, caller: Address) -> Result<u32, GovernanceError> {
        revoke_by_public_action_key(&env, &caller, GovernanceActionKey::Allocator)
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

    pub fn guardian(env: Env) -> Option<Address> {
        extend_instance_ttl(&env);
        env.storage().instance().get(&DataKey::Guardian)
    }

    pub fn sentinel(env: Env) -> Option<Address> {
        extend_instance_ttl(&env);
        env.storage().instance().get(&DataKey::Sentinel)
    }

    pub fn extend_ttl(env: Env, caller: Address) -> Result<(), GovernanceError> {
        require_admin(&env, &caller)?;
        extend_instance_ttl(&env);
        Ok(())
    }

    fn submit(env: Env, caller: Address, action: GovernanceAction) -> Result<u64, GovernanceError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        require_not_abdicated(&env, &action)?;
        validate_action(&action)?;

        let id = next_proposal_id(&env)?;
        let decision = decide_submission(&env, &action)?;
        let now_ns = ledger_timestamp_ns(&env)?;
        let timelock_ns = DurationNs(load_timelocks(&env).get(timelock_kind_for_action(&action)));
        let mut queue = load_queue(&env);
        let scheduled = queue.schedule_replacing(
            &action.pending_key(),
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

        if matches!(decision, TimelockDecision::Immediate) {
            let _removed =
                queue.revoke_by_key(&ProposalKey::ProposalId(id), queued_proposal_key_by_id);
            save_queue(&env, &queue);
            execute_action(&env, &action)?;
            ProposalSubmitted {
                id,
                valid_after_ns: 0,
            }
            .publish(&env);
            ProposalAccepted { id }.publish(&env);
            return Ok(id);
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
        GovernanceAction::SetPaused(_) => GovernanceActionKind::Pause,
        GovernanceAction::SetCurator(_) => GovernanceActionKind::Curator,
        GovernanceAction::SetGovernance(_) => GovernanceActionKind::Governance,
        GovernanceAction::SetSupplyQueue(_) => GovernanceActionKind::SupplyQueue,
        GovernanceAction::SetFees(_) => GovernanceActionKind::Fees,
        GovernanceAction::SetRestrictions(_, _) => GovernanceActionKind::Restrictions,
        GovernanceAction::SetGuardian(_) => GovernanceActionKind::Guardian,
        GovernanceAction::SetSentinel(_) => GovernanceActionKind::Sentinel,
        GovernanceAction::SetCap(_, _) => GovernanceActionKind::Cap,
        GovernanceAction::RemoveMarket(_) => GovernanceActionKind::MarketRemoval,
        GovernanceAction::SetGroupCap(_, _)
        | GovernanceAction::SetGroupRelCap(_, _)
        | GovernanceAction::SetGroupMember(_, _) => GovernanceActionKind::CapGroup,
        GovernanceAction::SetSkimRecipient(_) | GovernanceAction::Skim(_) => {
            GovernanceActionKind::Skim
        }
        GovernanceAction::SetAllocators(_) => GovernanceActionKind::Allocator,
        GovernanceAction::SetTimelock(_, _) => GovernanceActionKind::TimelockConfig,
        GovernanceAction::Other(_, _) => GovernanceActionKind::Other,
    }
}

fn timelock_kind_for_action(action: &GovernanceAction) -> TimelockKind {
    match action {
        GovernanceAction::SetPaused(_) => TimelockKind::Pause,
        GovernanceAction::SetCurator(_) => TimelockKind::Curator,
        GovernanceAction::SetGovernance(_) => TimelockKind::Governance,
        GovernanceAction::SetSupplyQueue(_) => TimelockKind::SupplyQueue,
        GovernanceAction::SetFees(_) => TimelockKind::Fees,
        GovernanceAction::SetRestrictions(_, _) => TimelockKind::Restrictions,
        GovernanceAction::SetGuardian(_) => TimelockKind::Guardian,
        GovernanceAction::SetSentinel(_) => TimelockKind::Sentinel,
        GovernanceAction::SetCap(_, _) => TimelockKind::Cap,
        GovernanceAction::RemoveMarket(_) => TimelockKind::MarketRemoval,
        GovernanceAction::SetGroupCap(_, _)
        | GovernanceAction::SetGroupRelCap(_, _)
        | GovernanceAction::SetGroupMember(_, _) => TimelockKind::CapGroup,
        GovernanceAction::SetSkimRecipient(_) | GovernanceAction::Skim(_) => TimelockKind::Skim,
        GovernanceAction::SetAllocators(_) => TimelockKind::Allocator,
        GovernanceAction::SetTimelock(_, _) => TimelockKind::TimelockConfig,
        GovernanceAction::Other(_, _) => TimelockKind::Other,
    }
}

fn validate_action(action: &GovernanceAction) -> Result<(), GovernanceError> {
    match action {
        GovernanceAction::SetGovernance(governance) => require_contract_address(governance),
        GovernanceAction::SetFees(params) => {
            let _ = to_wad(params.performance_fee_wad)?;
            let _ = to_wad(params.management_fee_wad)?;
            if let Some(max_rate) = params.max_growth_rate_wad {
                let _ = to_wad(max_rate)?;
            }
            Ok(())
        }
        GovernanceAction::SetCap(_, new_cap) => {
            let _ = to_wad(*new_cap)?;
            Ok(())
        }
        GovernanceAction::SetGroupCap(_, new_cap) => {
            if let Some(new_cap) = new_cap {
                let _ = to_wad(*new_cap)?;
            }
            Ok(())
        }
        GovernanceAction::SetGroupRelCap(_, new_relative_cap_wad) => {
            if let Some(new_relative_cap_wad) = new_relative_cap_wad {
                let relative = to_wad(*new_relative_cap_wad)?;
                if relative > Wad::one() {
                    return Err(GovernanceError::InvalidInput);
                }
            }
            Ok(())
        }
        GovernanceAction::SetTimelock(_, new_timelock_ns) => validate_timelock_ns(*new_timelock_ns),
        GovernanceAction::SetAllocators(_) | GovernanceAction::Other(_, _) => Ok(()),
        _ => Ok(()),
    }
}

fn decide_submission(
    env: &Env,
    action: &GovernanceAction,
) -> Result<TimelockDecision, GovernanceError> {
    match action {
        GovernanceAction::SetPaused(paused) => {
            let current = env
                .storage()
                .instance()
                .get(&DataKey::CurrentPaused)
                .unwrap_or(false);
            if *paused == current {
                return Err(GovernanceError::NoChange);
            }
            if *paused {
                Ok(TimelockDecision::Immediate)
            } else {
                Ok(TimelockDecision::Timelocked)
            }
        }
        GovernanceAction::SetGuardian(next) => {
            let current: Option<Address> = env.storage().instance().get(&DataKey::Guardian);
            if current.as_ref() == Some(next) {
                return Err(GovernanceError::NoChange);
            }
            Ok(TimelockDecision::from_requires_timelock(current.is_some()))
        }
        GovernanceAction::SetSentinel(next) => {
            let current: Option<Address> = env.storage().instance().get(&DataKey::Sentinel);
            if current.as_ref() == Some(next) {
                return Err(GovernanceError::NoChange);
            }
            Ok(TimelockDecision::from_requires_timelock(current.is_some()))
        }
        GovernanceAction::SetTimelock(kind, proposed) => {
            decide_timelock_submission(env, *kind, *proposed)
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

            let current_restrictions = to_shared_restrictions(current_mode, &current_accounts);
            let proposed_restrictions = to_shared_restrictions(*mode, accounts);

            if SharedRestrictions::determine_relaxed(&current_restrictions, &proposed_restrictions)
            {
                Ok(TimelockDecision::Timelocked)
            } else {
                Ok(TimelockDecision::Immediate)
            }
        }
        GovernanceAction::SetCap(_, new_cap) => {
            let decision = TimelockDecision::from_cap_change(None, to_wad(*new_cap)?.into());
            match decision {
                Ok(TimelockDecision::Immediate) => Ok(TimelockDecision::Immediate),
                Ok(TimelockDecision::Timelocked) => Ok(TimelockDecision::Timelocked),
                Err(CapChangeError::NoChange) => Err(GovernanceError::NoChange),
            }
        }
        GovernanceAction::RemoveMarket(_) => Ok(TimelockDecision::from_requires_timelock(true)),
        GovernanceAction::SetGroupCap(_, new_cap) => {
            let current = current_group_cap(env, action)?;
            let proposed = match new_cap {
                Some(value) => Some(to_wad(*value)?.into()),
                None => None,
            };
            let decision = TimelockDecision::from_cap_group_cap_change(current, proposed);
            match decision {
                Ok(TimelockDecision::Immediate) => Ok(TimelockDecision::Immediate),
                Ok(TimelockDecision::Timelocked) => Ok(TimelockDecision::Timelocked),
                Err(CapChangeError::NoChange) => Err(GovernanceError::NoChange),
            }
        }
        GovernanceAction::SetGroupRelCap(_, new_relative_cap_wad) => {
            let current = current_group_rel_cap(env, action)?;
            let proposed = match new_relative_cap_wad {
                Some(value) => Some(to_wad(*value)?),
                None => None,
            };
            match TimelockDecision::from_relative_cap_change(current, proposed) {
                Ok(decision) => Ok(decision),
                Err(RelativeCapChangeError::NoChange) => Err(GovernanceError::NoChange),
                Err(RelativeCapChangeError::RelativeCapTooHigh) => {
                    Err(GovernanceError::InvalidInput)
                }
            }
        }
        GovernanceAction::SetGroupMember(_, cap_group_id) => {
            let current = current_group_member(env, action);
            let proposed = cap_group_id.as_ref();
            match TimelockDecision::from_membership_assignment_change::<SdkString>(
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
        GovernanceAction::Skim(_) => Ok(TimelockDecision::Immediate),
        GovernanceAction::SetAllocators(proposed) => {
            let current: Vec<Address> = env
                .storage()
                .instance()
                .get(&DataKey::CurrentAllocators)
                .unwrap_or_else(|| Vec::new(env));
            if current == *proposed {
                return Err(GovernanceError::NoChange);
            }
            Ok(TimelockDecision::Timelocked)
        }
        GovernanceAction::SetCurator(_)
        | GovernanceAction::SetGovernance(_)
        | GovernanceAction::SetSupplyQueue(_) => Ok(TimelockDecision::Timelocked),
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

fn decide_timelock_submission(
    env: &Env,
    scope: TimelockScope,
    proposed: u64,
) -> Result<TimelockDecision, GovernanceError> {
    let timelocks = load_timelocks(env);
    if let TimelockScope::One(kind) = scope {
        return timelock_change_decision(timelocks.get(kind), proposed);
    }

    let mut changed = false;
    let mut timelocked = false;
    for kind in all_timelock_kinds() {
        match timelock_change_decision(timelocks.get(kind), proposed) {
            Ok(TimelockDecision::Immediate) => changed = true,
            Ok(TimelockDecision::Timelocked) => {
                changed = true;
                timelocked = true;
            }
            Err(GovernanceError::NoChange) => {}
            Err(err) => return Err(err),
        }
    }

    if !changed {
        return Err(GovernanceError::NoChange);
    }

    Ok(if timelocked {
        TimelockDecision::Timelocked
    } else {
        TimelockDecision::Immediate
    })
}

fn timelock_change_decision(
    current: u64,
    proposed: u64,
) -> Result<TimelockDecision, GovernanceError> {
    timelock_config_decision(
        DurationNs(current),
        DurationNs(proposed),
        DurationNs(MIN_TIMELOCK_NS),
        DurationNs(MAX_TIMELOCK_NS),
    )
    .map_err(|err| match err {
        TimelockConfigError::NoChange => GovernanceError::NoChange,
        TimelockConfigError::OutOfBounds => GovernanceError::TimelockOutOfBounds,
    })
}

fn all_timelock_kinds() -> [TimelockKind; 15] {
    [
        TimelockKind::Pause,
        TimelockKind::Curator,
        TimelockKind::Governance,
        TimelockKind::SupplyQueue,
        TimelockKind::Fees,
        TimelockKind::Restrictions,
        TimelockKind::Guardian,
        TimelockKind::Sentinel,
        TimelockKind::Cap,
        TimelockKind::MarketRemoval,
        TimelockKind::CapGroup,
        TimelockKind::Skim,
        TimelockKind::Allocator,
        TimelockKind::TimelockConfig,
        TimelockKind::Other,
    ]
}

fn set_all_timelocks(timelocks: &mut Timelocks, value: u64) {
    for kind in all_timelock_kinds() {
        timelocks.set(kind, value);
    }
}

fn current_group_cap(
    env: &Env,
    action: &GovernanceAction,
) -> Result<Option<u128>, GovernanceError> {
    let GovernanceAction::SetGroupCap(cap_group_id, _) = action else {
        return Err(GovernanceError::InvalidInput);
    };
    let current: Option<i128> = env
        .storage()
        .instance()
        .get(&DataKey::CurrentGroupCap(cap_group_id.clone()));
    current
        .map(to_wad)
        .transpose()
        .map(|value| value.map(Into::into))
}

fn current_group_rel_cap(
    env: &Env,
    action: &GovernanceAction,
) -> Result<Option<Wad>, GovernanceError> {
    let GovernanceAction::SetGroupRelCap(cap_group_id, _) = action else {
        return Err(GovernanceError::InvalidInput);
    };
    let current: Option<i128> = env
        .storage()
        .instance()
        .get(&DataKey::CurrentGroupRelCap(cap_group_id.clone()));
    current.map(to_wad).transpose()
}

fn current_group_member(env: &Env, action: &GovernanceAction) -> Option<SdkString> {
    let GovernanceAction::SetGroupMember(market_id, _) = action else {
        return None;
    };
    env.storage()
        .instance()
        .get(&DataKey::CurrentGroupMember(*market_id))
}

fn update_current_cap_group_state(
    env: &Env,
    action: &GovernanceAction,
) -> Result<(), GovernanceError> {
    match action {
        GovernanceAction::SetGroupCap(cap_group_id, value) => {
            let key = DataKey::CurrentGroupCap(cap_group_id.clone());
            if let Some(value) = value {
                env.storage().instance().set(&key, value);
            } else {
                env.storage().instance().remove(&key);
            }
        }
        GovernanceAction::SetGroupRelCap(cap_group_id, value) => {
            let key = DataKey::CurrentGroupRelCap(cap_group_id.clone());
            if let Some(value) = value {
                env.storage().instance().set(&key, value);
            } else {
                env.storage().instance().remove(&key);
            }
        }
        GovernanceAction::SetGroupMember(market_id, cap_group_id) => {
            let key = DataKey::CurrentGroupMember(*market_id);
            if let Some(cap_group_id) = cap_group_id {
                env.storage().instance().set(&key, cap_group_id);
            } else {
                env.storage().instance().remove(&key);
            }
        }
        GovernanceAction::SetCap(_, _) | GovernanceAction::RemoveMarket(_) => {}
        _ => return Err(GovernanceError::InvalidInput),
    }
    Ok(())
}

fn to_shared_restrictions(
    mode: RestrictionMode,
    accounts: &Vec<Address>,
) -> Option<SharedRestrictions<Address>> {
    match mode {
        RestrictionMode::None => None,
        RestrictionMode::Blacklist => {
            Some(SharedRestrictions::Blacklist(accounts_to_vec(accounts)))
        }
        RestrictionMode::Whitelist => {
            Some(SharedRestrictions::Whitelist(accounts_to_vec(accounts)))
        }
    }
}

fn accounts_to_vec(accounts: &Vec<Address>) -> alloc::vec::Vec<Address> {
    let mut deduped = alloc::vec::Vec::new();
    for account in accounts.iter() {
        if !deduped.iter().any(|existing| existing == &account) {
            deduped.push(account.clone());
        }
    }
    deduped
}

fn dedup_addresses(env: &Env, accounts: &Vec<Address>) -> Vec<Address> {
    let mut deduped = Vec::new(env);
    for account in accounts.iter() {
        if !deduped.iter().any(|existing| existing == account) {
            deduped.push_back(account);
        }
    }
    deduped
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
    let timelocks = Timelocks::from_default(default_ns);
    env.storage()
        .instance()
        .set(&DataKey::Timelocks, &timelocks);
    timelocks
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

fn load_queue(env: &Env) -> PendingActions<QueuedProposal> {
    let stored: Vec<StoredPending> = env
        .storage()
        .instance()
        .get(&DataKey::PendingQueue)
        .unwrap_or_else(|| Vec::new(env));

    let mut entries = alloc::vec::Vec::new();
    for item in stored.iter() {
        entries.push(PendingValue {
            value: QueuedProposal {
                id: item.id,
                action: item.action.clone(),
            },
            ready_at_ns: TimestampNs(item.valid_at_ns),
        });
    }

    PendingActions::from_restored_entries(entries)
}

fn save_queue(env: &Env, queue: &PendingActions<QueuedProposal>) {
    let mut stored = Vec::new(env);
    for entry in queue.iter() {
        stored.push_back(StoredPending {
            id: entry.value.id,
            action: entry.value.action.clone(),
            valid_at_ns: entry.ready_at_ns.into(),
        });
    }
    env.storage()
        .instance()
        .set(&DataKey::PendingQueue, &stored);
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

fn queued_proposal_action_key(proposal: &QueuedProposal) -> ProposalKey {
    ProposalKey::Action(proposal.action_key())
}

fn accept_by_action_key(
    env: &Env,
    caller: &Address,
    key: GovernanceActionKey,
) -> Result<u64, GovernanceError> {
    extend_instance_ttl(env);
    require_admin(env, caller)?;

    let now_ns = ledger_timestamp_ns(env)?;
    let mut queue = load_queue(env);
    let proposal = match queue.take_by_key(
        now_ns,
        &ProposalKey::Action(key),
        queued_proposal_action_key,
    ) {
        TakePending::Ready(proposal) => proposal,
        TakePending::Missing => return Err(GovernanceError::ProposalNotFound),
        TakePending::Pending { .. } => return Err(GovernanceError::ProposalNotMature),
    };

    execute_action(env, &proposal.action)?;
    save_queue(env, &queue);
    ProposalAccepted { id: proposal.id }.publish(env);
    Ok(proposal.id)
}

fn revoke_by_public_action_key(
    env: &Env,
    caller: &Address,
    key: GovernanceActionKey,
) -> Result<u32, GovernanceError> {
    extend_instance_ttl(env);
    require_revoker(env, caller)?;
    let removed = revoke_by_action_key(env, &key);
    if removed == 0 {
        return Err(GovernanceError::ProposalNotFound);
    }
    Ok(removed)
}

fn cap_group_key_to_action_key(key: CapGroupUpdateKey) -> GovernanceActionKey {
    match key {
        CapGroupUpdateKey::Cap(cap_group_id) => GovernanceActionKey::CapGroupCap(cap_group_id),
        CapGroupUpdateKey::RelativeCap(cap_group_id) => {
            GovernanceActionKey::CapGroupRelativeCap(cap_group_id)
        }
        CapGroupUpdateKey::Member(market_id) => GovernanceActionKey::CapGroupMembership(market_id),
    }
}

fn timelock_scope(kind: Option<TimelockKind>) -> TimelockScope {
    match kind {
        Some(kind) => TimelockScope::One(kind),
        None => TimelockScope::All,
    }
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

fn revoke_where(env: &Env, pred: impl Fn(&GovernanceAction) -> bool) -> u32 {
    let mut queue = load_queue(env);
    let mut revoked_ids = Vec::new(env);
    let mut keys = alloc::vec::Vec::new();

    for entry in queue.iter() {
        if pred(&entry.value.action) {
            revoked_ids.push_back(entry.value.id);
            let key = entry.value.action_key();
            if !keys.iter().any(|existing| existing == &key) {
                keys.push(key);
            }
        }
    }

    if revoked_ids.is_empty() {
        return 0;
    }

    for key in keys.iter() {
        let _removed = queue.revoke_by_key(key, QueuedProposal::action_key);
    }

    save_queue(env, &queue);

    for id in revoked_ids.iter() {
        ProposalRevoked { id }.publish(env);
    }

    revoked_ids.len()
}

fn execute_action(env: &Env, action: &GovernanceAction) -> Result<(), GovernanceError> {
    let vault = get_address(env, DataKey::Vault)?;

    match action {
        GovernanceAction::SetPaused(paused) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage()
                .instance()
                .set(&DataKey::CurrentPaused, paused);
        }
        GovernanceAction::SetCurator(_)
        | GovernanceAction::SetGovernance(_)
        | GovernanceAction::SetSupplyQueue(_) => {
            execute_vault_governance_action(env, &vault, action)?
        }
        GovernanceAction::SetFees(params) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage().instance().set(&DataKey::CurrentFees, params);
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
        GovernanceAction::SetGuardian(guardian) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage().instance().set(&DataKey::Guardian, guardian);
        }
        GovernanceAction::SetSentinel(sentinel) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage().instance().set(&DataKey::Sentinel, sentinel);
        }
        GovernanceAction::SetCap(_, _)
        | GovernanceAction::RemoveMarket(_)
        | GovernanceAction::SetGroupCap(_, _)
        | GovernanceAction::SetGroupRelCap(_, _)
        | GovernanceAction::SetGroupMember(_, _) => {
            execute_vault_governance_action(env, &vault, action)?;
            update_current_cap_group_state(env, action)?;
        }
        GovernanceAction::SetSkimRecipient(recipient) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage()
                .instance()
                .set(&DataKey::SkimRecipient, recipient);
        }
        GovernanceAction::Skim(_) => execute_vault_governance_action(env, &vault, action)?,
        GovernanceAction::SetAllocators(allocators) => {
            execute_vault_governance_action(env, &vault, action)?;
            env.storage()
                .instance()
                .set(&DataKey::CurrentAllocators, allocators);
        }
        GovernanceAction::SetTimelock(kind, new_timelock_ns) => {
            validate_timelock_ns(*new_timelock_ns)?;
            let mut timelocks = load_timelocks(env);
            match kind {
                TimelockScope::One(kind) => timelocks.set(*kind, *new_timelock_ns),
                TimelockScope::All => set_all_timelocks(&mut timelocks, *new_timelock_ns),
            }
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
        GovernanceAction::SetGuardian(guardian) => Some(GovernanceCommand::SetGovernanceConfig {
            kind: GOVERNANCE_CONFIG_KIND_GUARDIANS,
            primary: None,
            many: Some(alloc::vec![sdk_address_to_alloc_string(guardian)?]),
            value_a: None,
            value_b: None,
        }),
        GovernanceAction::SetSentinel(sentinel) => Some(GovernanceCommand::SetGovernanceConfig {
            kind: GOVERNANCE_CONFIG_KIND_SENTINEL,
            primary: Some(sdk_address_to_alloc_string(sentinel)?),
            many: None,
            value_a: None,
            value_b: None,
        }),
        GovernanceAction::SetSkimRecipient(recipient) => {
            Some(GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_SKIM_RECIPIENT,
                primary: Some(sdk_address_to_alloc_string(recipient)?),
                many: None,
                value_a: None,
                value_b: None,
            })
        }
        GovernanceAction::SetAllocators(allocators) => {
            Some(GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_ALLOCATORS,
                primary: None,
                many: Some(soroban_address_vec_to_alloc(allocators)?),
                value_a: None,
                value_b: None,
            })
        }
        GovernanceAction::Skim(token) => Some(GovernanceCommand::Skim {
            token: sdk_address_to_alloc_string(token)?,
        }),
        GovernanceAction::SetSupplyQueue(target_ids) => {
            Some(GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                target_ids: Some(soroban_u32_vec_to_alloc(target_ids)),
                mode: None,
                accounts: None,
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
                value: *cap,
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
                value: *relative_cap_wad,
                value_b: None,
                value_c: None,
            })
        }
        GovernanceAction::SetGroupMember(market_id, cap_group_id) => {
            let cap_group_id = match cap_group_id {
                Some(value) => Some(sdk_string_to_alloc_string(value)?),
                None => None,
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
        GovernanceAction::SetTimelock(_, _) | GovernanceAction::Other(_, _) => None,
    };

    Ok(command.map(|command| Bytes::from_slice(env, &command.encode())))
}

fn sdk_address_to_alloc_string(address: &Address) -> Result<AllocString, GovernanceError> {
    let raw = address.to_string().to_bytes().to_alloc_vec();
    AllocString::from_utf8(raw).map_err(|_| GovernanceError::InvalidInput)
}

fn sdk_string_to_alloc_string(value: &SdkString) -> Result<AllocString, GovernanceError> {
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

fn require_revoker(env: &Env, caller: &Address) -> Result<(), GovernanceError> {
    caller.require_auth();
    let admin = get_address(env, DataKey::Admin)?;
    if caller == &admin {
        return Ok(());
    }
    let guardian: Option<Address> = env.storage().instance().get(&DataKey::Guardian);
    if guardian.as_ref() == Some(caller) {
        return Ok(());
    }
    let sentinel: Option<Address> = env.storage().instance().get(&DataKey::Sentinel);
    if sentinel.as_ref() == Some(caller) {
        return Ok(());
    }
    Err(GovernanceError::Unauthorized)
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

fn ledger_timestamp_ns(env: &Env) -> Result<TimestampNs, GovernanceError> {
    seconds_to_nanoseconds(env.ledger().timestamp())
        .map(TimestampNs)
        .ok_or(GovernanceError::ArithmeticOverflow)
}

fn is_contract_address(addr: &Address) -> bool {
    let bytes = addr.to_string().to_bytes();
    matches!(bytes.get(0), Some(b'C'))
}

fn require_contract_address(addr: &Address) -> Result<(), GovernanceError> {
    if is_contract_address(addr) {
        Ok(())
    } else {
        Err(GovernanceError::InvalidInput)
    }
}

fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

#[cfg(test)]
mod tests;
