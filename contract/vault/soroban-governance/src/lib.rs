#![no_std]

extern crate alloc;

use alloc::collections::{BTreeSet, VecDeque};

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contracterror, contractevent, contractimpl, contracttype, Address, BytesN, Env,
    IntoVal, Symbol, Val, Vec,
};
use templar_curator_primitives::governance::{
    determine_relaxed, evaluate_fee_change, guardian_change_decision, queue_has_pending,
    queue_revoke_pending, queue_schedule, queue_take_mature, sentinel_change_decision,
    timelock_config_decision, FeeChangeError, FeeConfig, PendingQueueError, PendingValue,
    Restrictions as SharedRestrictions, TimelockConfigError, TimelockDecision,
};
use templar_vault_kernel::math::wad::Wad;

const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
const INSTANCE_TTL_EXTEND_TO: u32 = 3_110_400;
const MIN_TIMELOCK_NS: u64 = 0;
const MAX_TIMELOCK_NS: u64 = u64::MAX;

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Guardian,
    Sentinel,
    Vault,
    TimelockNs,
    Timelocks,
    NextProposalId,
    PendingQueue,
    ApprovedOther(Symbol, BytesN<32>),
    CurrentPaused,
    CurrentFees,
    CurrentRestrictionMode,
    CurrentRestrictionAccounts,
    ReentrancyLock,
}

#[contracttype]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum TimelockKind {
    Pause,
    Curator,
    Governance,
    SupplyQueue,
    Fees,
    Restrictions,
    Guardian,
    Sentinel,
    TimelockConfig,
    Other,
}

#[contracttype]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum GovernanceActionKind {
    Pause,
    Curator,
    Governance,
    SupplyQueue,
    Fees,
    Restrictions,
    Guardian,
    Sentinel,
    TimelockConfig,
    Other,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct Timelocks {
    pub pause_ns: u64,
    pub curator_ns: u64,
    pub governance_ns: u64,
    pub supply_queue_ns: u64,
    pub fees_ns: u64,
    pub restrictions_ns: u64,
    pub guardian_ns: u64,
    pub sentinel_ns: u64,
    pub timelock_config_ns: u64,
    pub other_ns: u64,
}

impl Timelocks {
    fn from_default(default_ns: u64) -> Self {
        Self {
            pause_ns: default_ns,
            curator_ns: default_ns,
            governance_ns: default_ns,
            supply_queue_ns: default_ns,
            fees_ns: default_ns,
            restrictions_ns: default_ns,
            guardian_ns: default_ns,
            sentinel_ns: default_ns,
            timelock_config_ns: default_ns,
            other_ns: default_ns,
        }
    }

    fn get(self, kind: TimelockKind) -> u64 {
        match kind {
            TimelockKind::Pause => self.pause_ns,
            TimelockKind::Curator => self.curator_ns,
            TimelockKind::Governance => self.governance_ns,
            TimelockKind::SupplyQueue => self.supply_queue_ns,
            TimelockKind::Fees => self.fees_ns,
            TimelockKind::Restrictions => self.restrictions_ns,
            TimelockKind::Guardian => self.guardian_ns,
            TimelockKind::Sentinel => self.sentinel_ns,
            TimelockKind::TimelockConfig => self.timelock_config_ns,
            TimelockKind::Other => self.other_ns,
        }
    }

    fn set(&mut self, kind: TimelockKind, value: u64) {
        match kind {
            TimelockKind::Pause => self.pause_ns = value,
            TimelockKind::Curator => self.curator_ns = value,
            TimelockKind::Governance => self.governance_ns = value,
            TimelockKind::SupplyQueue => self.supply_queue_ns = value,
            TimelockKind::Fees => self.fees_ns = value,
            TimelockKind::Restrictions => self.restrictions_ns = value,
            TimelockKind::Guardian => self.guardian_ns = value,
            TimelockKind::Sentinel => self.sentinel_ns = value,
            TimelockKind::TimelockConfig => self.timelock_config_ns = value,
            TimelockKind::Other => self.other_ns = value,
        }
    }
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct FeeParams {
    pub performance_fee_wad: i128,
    pub performance_recipient: Address,
    pub management_fee_wad: i128,
    pub management_recipient: Address,
    pub max_growth_rate_wad: Option<i128>,
}

#[contracttype]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum RestrictionMode {
    None,
    Paused,
    Blacklist,
    Whitelist,
}

impl RestrictionMode {
    fn from_u32(value: u32) -> Result<Self, GovernanceError> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Paused),
            2 => Ok(Self::Blacklist),
            3 => Ok(Self::Whitelist),
            _ => Err(GovernanceError::InvalidInput),
        }
    }

    fn as_u32(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Paused => 1,
            Self::Blacklist => 2,
            Self::Whitelist => 3,
        }
    }
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub enum GovernanceAction {
    SetPaused(bool),
    SetCurator(Address),
    SetGovernance(Address),
    SetSupplyQueue(Vec<u32>),
    SetFees(FeeParams),
    SetRestrictions(RestrictionMode, Vec<Address>),
    SetGuardian(Address),
    SetSentinel(Address),
    SetTimelock(TimelockKind, u64),
    Other(Symbol, BytesN<32>),
}

#[contracttype]
#[derive(Clone)]
pub struct PendingProposal {
    pub id: u64,
    pub action: GovernanceAction,
    pub valid_after_ns: u64,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct QueuedProposal {
    pub id: u64,
    pub action: GovernanceAction,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct StoredPending {
    pub id: u64,
    pub action: GovernanceAction,
    pub valid_at_ns: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct ProposalSubmitted {
    #[topic]
    pub id: u64,
    pub valid_after_ns: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct ProposalAccepted {
    #[topic]
    pub id: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct ProposalRevoked {
    #[topic]
    pub id: u64,
}

#[contracterror]
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub enum GovernanceError {
    Unauthorized = 1,
    InvalidInput = 2,
    MissingConfig = 3,
    ProposalNotFound = 4,
    ProposalNotMature = 5,
    Reentrancy = 6,
    ArithmeticOverflow = 7,
    DuplicatePending = 8,
    NoChange = 9,
    TimelockOutOfBounds = 10,
    OtherNotApproved = 11,
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
            .set(&DataKey::ReentrancyLock, &false);
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
            GovernanceAction::SetTimelock(kind, new_timelock_ns),
        )
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
        let removed = revoke_where(
            &env,
            |action| matches!(action, GovernanceAction::Other(k, h) if *k == key_for_match && *h == hash_for_match),
        );
        if removed == 0 {
            return Err(GovernanceError::ProposalNotFound);
        }
        Ok(removed)
    }

    pub fn accept(env: Env, caller: Address, proposal_id: u64) -> Result<(), GovernanceError> {
        with_reentrancy_guard(&env, || {
            extend_instance_ttl(&env);
            require_admin(&env, &caller)?;

            let now_ns = ledger_timestamp_ns(&env)?;
            let mut queue = load_queue(&env);
            let proposal =
                match queue_take_mature(&mut queue, now_ns, |pending| pending.id == proposal_id) {
                    Ok(Some(proposal)) => proposal,
                    Ok(None) => return Err(GovernanceError::ProposalNotFound),
                    Err(PendingQueueError::NotMature) => {
                        return Err(GovernanceError::ProposalNotMature)
                    }
                };

            execute_action(&env, &proposal.action)?;
            save_queue(&env, &queue);
            ProposalAccepted { id: proposal_id }.publish(&env);
            Ok(())
        })
    }

    pub fn accept_kind(
        env: Env,
        caller: Address,
        kind: GovernanceActionKind,
    ) -> Result<u64, GovernanceError> {
        with_reentrancy_guard(&env, || {
            extend_instance_ttl(&env);
            require_admin(&env, &caller)?;
            let now_ns = ledger_timestamp_ns(&env)?;

            let mut queue = load_queue(&env);
            let proposal = match queue_take_mature(&mut queue, now_ns, |pending| {
                action_kind(&pending.action) == kind
            }) {
                Ok(Some(proposal)) => proposal,
                Ok(None) => return Err(GovernanceError::ProposalNotFound),
                Err(PendingQueueError::NotMature) => {
                    return Err(GovernanceError::ProposalNotMature)
                }
            };

            execute_action(&env, &proposal.action)?;
            save_queue(&env, &queue);
            ProposalAccepted { id: proposal.id }.publish(&env);
            Ok(proposal.id)
        })
    }

    pub fn revoke(env: Env, caller: Address, proposal_id: u64) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        require_revoker(&env, &caller)?;
        let mut queue = load_queue(&env);
        if !queue_revoke_pending(&mut queue, |pending| pending.id == proposal_id) {
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
        with_reentrancy_guard(&env, || {
            extend_instance_ttl(&env);
            require_admin(&env, &caller)?;
            validate_action(&action)?;

            let id = next_proposal_id(&env)?;
            let decision = decide_submission(&env, &action)?;

            if matches!(decision, TimelockDecision::Immediate) {
                execute_action(&env, &action)?;
                ProposalSubmitted {
                    id,
                    valid_after_ns: 0,
                }
                .publish(&env);
                ProposalAccepted { id }.publish(&env);
                return Ok(id);
            }

            if has_pending_action(&env, &action) {
                return Err(GovernanceError::DuplicatePending);
            }

            let now_ns = ledger_timestamp_ns(&env)?;
            let timelock_ns = load_timelocks(&env).get(timelock_kind_for_action(&action));
            let mut queue = load_queue(&env);
            queue_schedule(
                &mut queue,
                QueuedProposal { id, action },
                now_ns,
                timelock_ns,
            );
            let valid_after_ns = queue
                .back()
                .map(|pending| pending.valid_at_ns)
                .unwrap_or(now_ns);
            save_queue(&env, &queue);

            ProposalSubmitted { id, valid_after_ns }.publish(&env);
            Ok(id)
        })
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
        GovernanceAction::SetTimelock(_, new_timelock_ns) => validate_timelock_ns(*new_timelock_ns),
        GovernanceAction::Other(_, _) => Ok(()),
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
            Ok(guardian_change_decision(current.is_some()))
        }
        GovernanceAction::SetSentinel(next) => {
            let current: Option<Address> = env.storage().instance().get(&DataKey::Sentinel);
            if current.as_ref() == Some(next) {
                return Err(GovernanceError::NoChange);
            }
            Ok(sentinel_change_decision(current.is_some()))
        }
        GovernanceAction::SetTimelock(kind, proposed) => {
            let current = load_timelocks(env).get(*kind);
            timelock_config_decision(current, *proposed, MIN_TIMELOCK_NS, MAX_TIMELOCK_NS).map_err(
                |err| match err {
                    TimelockConfigError::NoChange => GovernanceError::NoChange,
                    TimelockConfigError::OutOfBounds => GovernanceError::TimelockOutOfBounds,
                },
            )
        }
        GovernanceAction::SetFees(proposed) => {
            let current: FeeParams = env
                .storage()
                .instance()
                .get(&DataKey::CurrentFees)
                .ok_or(GovernanceError::MissingConfig)?;

            let current_cfg = FeeConfig::new(
                to_wad(current.performance_fee_wad)?,
                to_wad(current.management_fee_wad)?,
                &current.performance_recipient,
                &current.management_recipient,
                to_optional_wad(current.max_growth_rate_wad)?,
            );
            let proposed_cfg = FeeConfig::new(
                to_wad(proposed.performance_fee_wad)?,
                to_wad(proposed.management_fee_wad)?,
                &proposed.performance_recipient,
                &proposed.management_recipient,
                to_optional_wad(proposed.max_growth_rate_wad)?,
            );
            let decision =
                evaluate_fee_change(&current_cfg, &proposed_cfg).map_err(|err| match err {
                    FeeChangeError::NoChange => GovernanceError::NoChange,
                    FeeChangeError::PerformanceFeeTooHigh
                    | FeeChangeError::ManagementFeeTooHigh => GovernanceError::InvalidInput,
                })?;

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

            if determine_relaxed(&current_restrictions, &proposed_restrictions) {
                Ok(TimelockDecision::Timelocked)
            } else {
                Ok(TimelockDecision::Immediate)
            }
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

fn to_shared_restrictions(
    mode: RestrictionMode,
    accounts: &Vec<Address>,
) -> Option<SharedRestrictions<Address>> {
    match mode {
        RestrictionMode::None => None,
        RestrictionMode::Paused => Some(SharedRestrictions::Paused),
        RestrictionMode::Blacklist => {
            Some(SharedRestrictions::Blacklist(accounts_to_set(accounts)))
        }
        RestrictionMode::Whitelist => {
            Some(SharedRestrictions::Whitelist(accounts_to_set(accounts)))
        }
    }
}

fn accounts_to_set(accounts: &Vec<Address>) -> BTreeSet<Address> {
    let mut set = BTreeSet::new();
    for account in accounts.iter() {
        set.insert(account);
    }
    set
}

fn to_wad(value: i128) -> Result<Wad, GovernanceError> {
    if value < 0 {
        return Err(GovernanceError::InvalidInput);
    }
    Ok(Wad::from(value as u128))
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

fn load_queue(env: &Env) -> VecDeque<PendingValue<QueuedProposal>> {
    let stored: Vec<StoredPending> = env
        .storage()
        .instance()
        .get(&DataKey::PendingQueue)
        .unwrap_or_else(|| Vec::new(env));

    let mut queue = VecDeque::new();
    for item in stored.iter() {
        queue.push_back(PendingValue::new(
            QueuedProposal {
                id: item.id,
                action: item.action.clone(),
            },
            item.valid_at_ns,
        ));
    }

    queue
}

fn save_queue(env: &Env, queue: &VecDeque<PendingValue<QueuedProposal>>) {
    let mut stored = Vec::new(env);
    for entry in queue.iter() {
        stored.push_back(StoredPending {
            id: entry.value.id,
            action: entry.value.action.clone(),
            valid_at_ns: entry.valid_at_ns,
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
                valid_after_ns: entry.valid_at_ns,
            });
        }
    }
    Err(GovernanceError::ProposalNotFound)
}

fn has_pending_action(env: &Env, action: &GovernanceAction) -> bool {
    let queue = load_queue(env);
    queue_has_pending(&queue, |pending| pending.action == *action)
}

fn revoke_where(env: &Env, pred: impl Fn(&GovernanceAction) -> bool) -> u32 {
    let mut queue = load_queue(env);
    let mut revoked_ids = Vec::new(env);

    for entry in queue.iter() {
        if pred(&entry.value.action) {
            revoked_ids.push_back(entry.value.id);
        }
    }

    if revoked_ids.is_empty() {
        return 0;
    }

    let _removed = queue_revoke_pending(&mut queue, |pending| pred(&pending.action));
    save_queue(env, &queue);

    for id in revoked_ids.iter() {
        ProposalRevoked { id }.publish(env);
    }

    revoked_ids.len()
}

fn execute_action(env: &Env, action: &GovernanceAction) -> Result<(), GovernanceError> {
    let governance = env.current_contract_address();
    let vault = get_address(env, DataKey::Vault)?;

    match action {
        GovernanceAction::SetPaused(paused) => {
            let fn_name = Symbol::new(env, "set_paused");
            let args = (governance.clone(), *paused);
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
            env.storage()
                .instance()
                .set(&DataKey::CurrentPaused, paused);
        }
        GovernanceAction::SetCurator(new_curator) => {
            let fn_name = Symbol::new(env, "set_curator");
            let args = (governance.clone(), new_curator.clone());
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
        }
        GovernanceAction::SetGovernance(new_governance) => {
            let fn_name = Symbol::new(env, "set_governance");
            let args = (governance.clone(), new_governance.clone());
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
        }
        GovernanceAction::SetSupplyQueue(target_ids) => {
            let fn_name = Symbol::new(env, "set_supply_queue");
            let args = (governance.clone(), target_ids.clone());
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
        }
        GovernanceAction::SetFees(params) => {
            let fn_name = Symbol::new(env, "set_fees");
            let args = (
                governance.clone(),
                params.performance_fee_wad,
                params.performance_recipient.clone(),
                params.management_fee_wad,
                params.management_recipient.clone(),
                params.max_growth_rate_wad,
            );
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
            env.storage().instance().set(&DataKey::CurrentFees, params);
        }
        GovernanceAction::SetRestrictions(mode, accounts) => {
            let fn_name = Symbol::new(env, "set_restrictions");
            let args = (governance.clone(), mode.as_u32(), accounts.clone());
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
            env.storage()
                .instance()
                .set(&DataKey::CurrentRestrictionMode, mode);
            env.storage()
                .instance()
                .set(&DataKey::CurrentRestrictionAccounts, accounts);
        }
        GovernanceAction::SetGuardian(guardian) => {
            env.storage().instance().set(&DataKey::Guardian, guardian);
        }
        GovernanceAction::SetSentinel(sentinel) => {
            env.storage().instance().set(&DataKey::Sentinel, sentinel);
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

fn authorize_and_invoke(env: &Env, vault: &Address, fn_name: &Symbol, args: Vec<Val>) {
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

    let _: () = env.invoke_contract(vault, fn_name, args);
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

fn ledger_timestamp_ns(env: &Env) -> Result<u64, GovernanceError> {
    env.ledger()
        .timestamp()
        .checked_mul(1_000_000_000)
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

fn with_reentrancy_guard<T>(
    env: &Env,
    f: impl FnOnce() -> Result<T, GovernanceError>,
) -> Result<T, GovernanceError> {
    let locked: bool = env
        .storage()
        .instance()
        .get(&DataKey::ReentrancyLock)
        .unwrap_or(false);
    if locked {
        return Err(GovernanceError::Reentrancy);
    }

    env.storage()
        .instance()
        .set(&DataKey::ReentrancyLock, &true);
    let result = f();
    env.storage()
        .instance()
        .set(&DataKey::ReentrancyLock, &false);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};

    #[contract]
    struct MockVault;

    #[contracttype]
    #[derive(Clone, Eq, PartialEq)]
    enum MockVaultKey {
        Paused,
    }

    #[contractimpl]
    impl MockVault {
        pub fn set_paused(env: Env, _caller: Address, paused: bool) {
            env.storage().instance().set(&MockVaultKey::Paused, &paused);
        }

        pub fn is_paused(env: Env) -> bool {
            env.storage()
                .instance()
                .get(&MockVaultKey::Paused)
                .unwrap_or(false)
        }

        pub fn set_curator(_env: Env, _caller: Address, _new_curator: Address) {}

        pub fn set_governance(_env: Env, _caller: Address, _governance: Address) {}

        pub fn set_supply_queue(_env: Env, _caller: Address, _target_ids: Vec<u32>) {}

        pub fn set_fees(
            _env: Env,
            _caller: Address,
            _performance_fee_wad: i128,
            _performance_recipient: Address,
            _management_fee_wad: i128,
            _management_recipient: Address,
            _max_growth_rate_wad: Option<i128>,
        ) {
        }

        pub fn set_restrictions(_env: Env, _caller: Address, _mode: u32, _accounts: Vec<Address>) {}
    }

    #[test]
    fn pause_immediate_unpause_timelocked() {
        let env = Env::default();
        env.mock_all_auths();

        env.ledger().set(LedgerInfo {
            timestamp: 100,
            protocol_version: 23,
            ..Default::default()
        });

        let admin = Address::generate(&env);
        let vault = env.register(MockVault, ());
        let governance = env.register(
            SorobanVaultGovernanceContract,
            (&admin, &vault, &(5_000_000_000u64)),
        );

        let pause_id = env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::submit_set_paused(env.clone(), admin.clone(), true)
                .unwrap()
        });
        assert_eq!(pause_id, 1);
        let paused = env.as_contract(&vault, || MockVault::is_paused(env.clone()));
        assert!(paused);
        let pending = env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::pending_ids(env.clone())
        });
        assert_eq!(pending.len(), 0);

        let unpause_id = env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::submit_set_paused(env.clone(), admin.clone(), false)
                .unwrap()
        });
        assert_eq!(unpause_id, 2);

        let early = env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), unpause_id)
        });
        assert_eq!(early, Err(GovernanceError::ProposalNotMature));

        env.ledger().set(LedgerInfo {
            timestamp: 106,
            protocol_version: 23,
            ..Default::default()
        });

        env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), unpause_id).unwrap()
        });
        let paused = env.as_contract(&vault, || MockVault::is_paused(env.clone()));
        assert!(!paused);
    }

    #[test]
    fn revoke_kind_removes_all_matching() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 100,
            protocol_version: 23,
            ..Default::default()
        });

        let admin = Address::generate(&env);
        let vault = env.register(MockVault, ());
        let governance = env.register(
            SorobanVaultGovernanceContract,
            (&admin, &vault, &(5_000_000_000u64)),
        );

        env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::submit_set_curator(
                env.clone(),
                admin.clone(),
                Address::generate(&env),
            )
            .unwrap();
        });
        env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::submit_set_curator(
                env.clone(),
                admin.clone(),
                Address::generate(&env),
            )
            .unwrap();
        });

        let removed = env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::revoke_kind(
                env.clone(),
                admin.clone(),
                GovernanceActionKind::Curator,
            )
            .unwrap()
        });
        assert_eq!(removed, 2);

        let pending = env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::pending_ids(env.clone())
        });
        assert_eq!(pending.len(), 0);
    }

    #[test]
    fn timelock_config_increase_immediate_decrease_timelocked() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 100,
            protocol_version: 23,
            ..Default::default()
        });

        let admin = Address::generate(&env);
        let vault = env.register(MockVault, ());
        let governance = env.register(
            SorobanVaultGovernanceContract,
            (&admin, &vault, &(5_000_000_000u64)),
        );

        env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::submit_set_timelock(
                env.clone(),
                admin.clone(),
                TimelockKind::Curator,
                6_000_000_000,
            )
            .unwrap();
        });

        let updated = env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::timelock_ns(env.clone(), TimelockKind::Curator)
        });
        assert_eq!(updated, 6_000_000_000);

        env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::submit_set_timelock(
                env.clone(),
                admin.clone(),
                TimelockKind::Curator,
                4_000_000_000,
            )
            .unwrap();
        });

        let pending = env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::pending_ids(env.clone())
        });
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn other_action_approval_and_consume() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 100,
            protocol_version: 23,
            ..Default::default()
        });

        let admin = Address::generate(&env);
        let vault = env.register(MockVault, ());
        let governance = env.register(
            SorobanVaultGovernanceContract,
            (&admin, &vault, &(5_000_000_000u64)),
        );

        let key = Symbol::new(&env, "market_remove");
        let payload_hash = BytesN::from_array(&env, &[7u8; 32]);

        let proposal_id = env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::submit_other(
                env.clone(),
                admin.clone(),
                key.clone(),
                payload_hash.clone(),
            )
            .unwrap()
        });

        env.ledger().set(LedgerInfo {
            timestamp: 106,
            protocol_version: 23,
            ..Default::default()
        });

        env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), proposal_id).unwrap()
        });

        let approved = env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::check_other(
                env.clone(),
                key.clone(),
                payload_hash.clone(),
            )
        });
        assert!(approved);

        let unauthorized = env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::consume_other(
                env.clone(),
                admin.clone(),
                key.clone(),
                payload_hash.clone(),
            )
        });
        assert_eq!(unauthorized, Err(GovernanceError::Unauthorized));

        env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::consume_other(
                env.clone(),
                vault.clone(),
                key.clone(),
                payload_hash.clone(),
            )
            .unwrap();
        });

        let approved_after = env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::check_other(env.clone(), key, payload_hash)
        });
        assert!(!approved_after);
    }
}
