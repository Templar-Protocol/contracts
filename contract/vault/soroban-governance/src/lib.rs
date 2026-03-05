#![no_std]

extern crate alloc;

mod types;
pub use types::*;

use alloc::collections::{BTreeSet, VecDeque};

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contractimpl, Address, BytesN, Env, IntoVal, String as SdkString, Symbol, Val, Vec,
};
use templar_curator_primitives::governance::{
    cap_change_decision, determine_relaxed, evaluate_fee_change, guardian_change_decision,
    market_removal_decision, membership_change_decision, queue_has_pending, queue_revoke_pending,
    queue_schedule, queue_take_mature, relative_cap_change_decision, sentinel_change_decision,
    timelock_config_decision, CapChangeError, FeeChangeError, FeeConfig, MembershipChangeError,
    PendingQueueError, PendingValue, RelativeCapChangeError, Restrictions as SharedRestrictions,
    TimelockConfigError, TimelockDecision,
};
use templar_curator_primitives::{nonnegative_i128_to_u128, seconds_to_nanoseconds};
use templar_vault_kernel::math::wad::Wad;

const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
const INSTANCE_TTL_EXTEND_TO: u32 = 3_110_400;
const MIN_TIMELOCK_NS: u64 = 0;
const MAX_TIMELOCK_NS: u64 = u64::MAX;

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
            GovernanceAction::SetGroupCap(cap_group_id, new_cap),
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
            GovernanceAction::SetGroupRelCap(cap_group_id, new_relative_cap_wad),
        )
    }

    pub fn submit_set_group_member(
        env: Env,
        caller: Address,
        market_id: u32,
        cap_group_id: SdkString,
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

    pub fn abdicate(env: Env, caller: Address, method_name: Symbol) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        env.storage()
            .instance()
            .set(&DataKey::Abdicated(method_name), &true);
        Ok(())
    }

    pub fn is_abdicated(env: Env, method_name: Symbol) -> bool {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::Abdicated(method_name))
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
            require_not_abdicated(&env, &action)?;
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
        GovernanceAction::SetCap(_, _) => GovernanceActionKind::Cap,
        GovernanceAction::RemoveMarket(_) => GovernanceActionKind::MarketRemoval,
        GovernanceAction::SetGroupCap(_, _)
        | GovernanceAction::SetGroupRelCap(_, _)
        | GovernanceAction::SetGroupMember(_, _) => GovernanceActionKind::CapGroup,
        GovernanceAction::SetSkimRecipient(_) | GovernanceAction::Skim(_) => {
            GovernanceActionKind::Skim
        }
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
        GovernanceAction::SetCap(_, new_cap) => {
            let decision = cap_change_decision(None, to_wad(*new_cap)?.into());
            match decision {
                Ok(TimelockDecision::Immediate) => Ok(TimelockDecision::Immediate),
                Ok(TimelockDecision::Timelocked) => Ok(TimelockDecision::Timelocked),
                Err(CapChangeError::NoChange) => Err(GovernanceError::NoChange),
            }
        }
        GovernanceAction::RemoveMarket(_) => Ok(market_removal_decision(1)),
        GovernanceAction::SetGroupCap(_, new_cap) => {
            let decision = cap_change_decision(None, to_wad(*new_cap)?.into());
            match decision {
                Ok(TimelockDecision::Immediate) => Ok(TimelockDecision::Immediate),
                Ok(TimelockDecision::Timelocked) => Ok(TimelockDecision::Timelocked),
                Err(CapChangeError::NoChange) => Err(GovernanceError::NoChange),
            }
        }
        GovernanceAction::SetGroupRelCap(_, new_relative_cap_wad) => {
            match relative_cap_change_decision(None, to_wad(*new_relative_cap_wad)?) {
                Ok(decision) => Ok(decision),
                Err(RelativeCapChangeError::NoChange) => Err(GovernanceError::NoChange),
                Err(RelativeCapChangeError::RelativeCapTooHigh) => {
                    Err(GovernanceError::InvalidInput)
                }
            }
        }
        GovernanceAction::SetGroupMember(_, cap_group_id) => {
            let changed = !cap_group_id.is_empty();
            match membership_change_decision(changed) {
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
            let fn_name = Symbol::new(env, "set_guardians");
            let args = (governance.clone(), Vec::from_array(env, [guardian.clone()]));
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
            env.storage().instance().set(&DataKey::Guardian, guardian);
        }
        GovernanceAction::SetSentinel(sentinel) => {
            let fn_name = Symbol::new(env, "set_sentinel");
            let args = (governance.clone(), sentinel.clone());
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
            env.storage().instance().set(&DataKey::Sentinel, sentinel);
        }
        GovernanceAction::SetCap(market_id, new_cap) => {
            let fn_name = Symbol::new(env, "set_cap");
            let args = (governance.clone(), *market_id, *new_cap);
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
        }
        GovernanceAction::RemoveMarket(market_id) => {
            let fn_name = Symbol::new(env, "remove_market");
            let args = (governance.clone(), *market_id);
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
        }
        GovernanceAction::SetGroupCap(cap_group_id, new_cap) => {
            let fn_name = Symbol::new(env, "set_group_cap");
            let args = (governance.clone(), cap_group_id.clone(), *new_cap);
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
        }
        GovernanceAction::SetGroupRelCap(cap_group_id, new_relative_cap_wad) => {
            let fn_name = Symbol::new(env, "set_group_rel_cap");
            let args = (
                governance.clone(),
                cap_group_id.clone(),
                *new_relative_cap_wad,
            );
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
        }
        GovernanceAction::SetGroupMember(market_id, cap_group_id) => {
            let fn_name = Symbol::new(env, "set_group_member");
            let args = (governance.clone(), *market_id, cap_group_id.clone());
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
        }
        GovernanceAction::SetSkimRecipient(recipient) => {
            let fn_name = Symbol::new(env, "set_skim_recipient");
            let args = (governance.clone(), recipient.clone());
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
            env.storage()
                .instance()
                .set(&DataKey::SkimRecipient, recipient);
        }
        GovernanceAction::Skim(token) => {
            let fn_name = Symbol::new(env, "skim");
            let args = (governance.clone(), token.clone());
            authorize_and_invoke(env, &vault, &fn_name, args.into_val(env));
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

fn method_name_for_action(env: &Env, action: &GovernanceAction) -> Symbol {
    match action {
        GovernanceAction::SetGuardian(_) => Symbol::new(env, "submit_guardian"),
        GovernanceAction::SetSentinel(_) => Symbol::new(env, "submit_sentinel"),
        GovernanceAction::SetFees(_) => Symbol::new(env, "set_fees"),
        GovernanceAction::SetRestrictions(_, _) => Symbol::new(env, "set_restrictions"),
        GovernanceAction::SetCap(_, _) => Symbol::new(env, "submit_cap"),
        GovernanceAction::RemoveMarket(_) => Symbol::new(env, "submit_market_removal"),
        GovernanceAction::SetGroupCap(_, _)
        | GovernanceAction::SetGroupRelCap(_, _)
        | GovernanceAction::SetGroupMember(_, _) => Symbol::new(env, "submit_cap_group_update"),
        GovernanceAction::SetSkimRecipient(_) => Symbol::new(env, "set_skim_recipient"),
        GovernanceAction::Skim(_) => Symbol::new(env, "skim"),
        GovernanceAction::SetPaused(_) => Symbol::new(env, "set_paused"),
        GovernanceAction::SetCurator(_) => Symbol::new(env, "set_curator"),
        GovernanceAction::SetGovernance(_) => Symbol::new(env, "set_governance"),
        GovernanceAction::SetSupplyQueue(_) => Symbol::new(env, "set_supply_queue"),
        GovernanceAction::SetTimelock(_, _) => Symbol::new(env, "submit_timelock"),
        GovernanceAction::Other(_, _) => Symbol::new(env, "other"),
    }
}

fn require_not_abdicated(env: &Env, action: &GovernanceAction) -> Result<(), GovernanceError> {
    let method_name = method_name_for_action(env, action);
    let abdicated: bool = env
        .storage()
        .instance()
        .get(&DataKey::Abdicated(method_name))
        .unwrap_or(false);
    if abdicated {
        return Err(GovernanceError::Abdicated);
    }
    Ok(())
}

fn ledger_timestamp_ns(env: &Env) -> Result<u64, GovernanceError> {
    seconds_to_nanoseconds(env.ledger().timestamp()).ok_or(GovernanceError::ArithmeticOverflow)
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
mod tests;
