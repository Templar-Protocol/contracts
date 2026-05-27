//! The proposal "engine": authorization, validation, TTL computation, and
//! the dispatch table that translates a `GovernanceAction` into a runtime
//! sub-invocation (or a local effect like setting a TTL or toggling a role).

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    Address, Env, IntoVal, Symbol, Val, Vec,
};
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_soroban_common::{
    validate_action, ContractError, GovernanceAction, GovernanceError, OperationKind, Role,
    TtlConfig, MAX_MANUAL_TRIP_METADATA_LEN, MAX_PROPOSAL_TTL_NS,
};

use crate::{
    events::ActionTtlSet,
    roles,
    storage::{load_ttls, save_ttls, DataKey},
};

pub fn now(env: &Env) -> Result<Nanoseconds, GovernanceError> {
    env.ledger()
        .timestamp()
        .checked_mul(1_000_000_000)
        .map(Nanoseconds::from_ns)
        .ok_or(GovernanceError::ArithmeticOverflow)
}

pub fn require_authorized(
    env: &Env,
    caller: &Address,
    action: &GovernanceAction,
) -> Result<(), GovernanceError> {
    caller.require_auth();
    let required = action.required_role();
    if roles::has(env, caller, Role::Admin) || roles::has(env, caller, required) {
        Ok(())
    } else {
        Err(GovernanceError::Unauthorized)
    }
}

pub fn validate_for_creator(
    caller: &Address,
    action: &GovernanceAction,
) -> Result<(), GovernanceError> {
    validate_action(action, MAX_MANUAL_TRIP_METADATA_LEN)?;
    if let GovernanceAction::SetManualTrip(actor, _, _, _) = action {
        if actor != caller {
            return Err(GovernanceError::InvalidInput);
        }
    }
    Ok(())
}

pub fn effective_ttl(
    ttls: &TtlConfig,
    action: &GovernanceAction,
    requested_ttl: u64,
) -> Result<u64, GovernanceError> {
    if requested_ttl > MAX_PROPOSAL_TTL_NS {
        return Err(GovernanceError::TtlExceedsMaximum);
    }
    let requested_ttl = Nanoseconds::from_ns(requested_ttl);
    // `SetActionTtl` is special: the proposal that *edits* a per-op TTL must
    // itself wait at least as long as the longest of (its own TTL,
    // target-op TTL), so we can't shorten a long-running action by quickly
    // re-tuning its TTL.
    let minimum = match action {
        GovernanceAction::SetActionTtl(kind, _) => {
            let set_action_ttl = ttls.get(OperationKind::SetActionTtl);
            let target_ttl = ttls.get(*kind);
            set_action_ttl.max(target_ttl)
        }
        _ => ttls.get(action.kind()),
    };
    let ttl = minimum.max(requested_ttl);
    if ttl.as_ns() > MAX_PROPOSAL_TTL_NS {
        return Err(GovernanceError::TtlExceedsMaximum);
    }
    Ok(ttl.as_ns())
}

// One arm per `GovernanceAction` variant — the body is necessarily long.
#[allow(clippy::too_many_lines)]
pub fn execute_action(env: &Env, action: &GovernanceAction) -> Result<(), GovernanceError> {
    let proxy: Address = env
        .storage()
        .instance()
        .get(&DataKey::ProxyOracle)
        .ok_or(GovernanceError::MissingConfig)?;
    match action {
        GovernanceAction::SetProxy(asset, config) => invoke_runtime_call(
            env,
            &proxy,
            "set_proxy",
            Vec::from_array(
                env,
                [asset.clone().into_val(env), config.clone().into_val(env)],
            ),
        ),
        GovernanceAction::RemoveProxy(asset) => invoke_runtime_call(
            env,
            &proxy,
            "remove_proxy",
            Vec::from_array(env, [asset.clone().into_val(env)]),
        ),
        GovernanceAction::ConfigureBreakers(asset, sample_interval_secs, history_len) => {
            invoke_runtime_call(
                env,
                &proxy,
                "configure_breakers",
                Vec::from_array(
                    env,
                    [
                        asset.clone().into_val(env),
                        sample_interval_secs.into_val(env),
                        history_len.into_val(env),
                    ],
                ),
            )
        }
        GovernanceAction::AddBreaker(asset, breaker) => invoke_runtime_call(
            env,
            &proxy,
            "add_breaker",
            Vec::from_array(
                env,
                [asset.clone().into_val(env), breaker.clone().into_val(env)],
            ),
        ),
        GovernanceAction::RemoveBreaker(asset, breaker_id) => invoke_runtime_call(
            env,
            &proxy,
            "remove_breaker",
            Vec::from_array(env, [asset.clone().into_val(env), breaker_id.into_val(env)]),
        ),
        GovernanceAction::Rearm(asset, breaker_id, config) => invoke_runtime_call(
            env,
            &proxy,
            "rearm",
            Vec::from_array(
                env,
                [
                    asset.clone().into_val(env),
                    breaker_id.into_val(env),
                    config.clone().into_val(env),
                ],
            ),
        ),
        GovernanceAction::SetEnforced(asset, breaker_id, config) => invoke_runtime_call(
            env,
            &proxy,
            "set_enforced",
            Vec::from_array(
                env,
                [
                    asset.clone().into_val(env),
                    breaker_id.into_val(env),
                    config.clone().into_val(env),
                ],
            ),
        ),
        GovernanceAction::SetManualTrip(actor, asset, is_manually_tripped, metadata) => {
            invoke_runtime_call(
                env,
                &proxy,
                "set_manual_trip",
                Vec::from_array(
                    env,
                    [
                        actor.clone().into_val(env),
                        asset.clone().into_val(env),
                        is_manually_tripped.into_val(env),
                        metadata.clone().into_val(env),
                    ],
                ),
            )
        }
        GovernanceAction::SetGovernance(governance) => invoke_runtime_call(
            env,
            &proxy,
            "set_governance",
            Vec::from_array(env, [governance.clone().into_val(env)]),
        ),
        GovernanceAction::SetActionTtl(kind, new_ttl_ns) => {
            let mut ttls = load_ttls(env)?;
            ttls.set(*kind, Nanoseconds::from_ns(*new_ttl_ns));
            save_ttls(env, &ttls);
            ActionTtlSet {
                kind: *kind,
                new_ttl_ns: *new_ttl_ns,
            }
            .publish(env);
            Ok(())
        }
        GovernanceAction::SetRole(account, role, set) => {
            if *set {
                roles::grant(env, account, *role);
                Ok(())
            } else {
                roles::revoke(env, account, *role)
            }
        }
        GovernanceAction::AdminUpgrade(new_wasm_hash) => invoke_runtime_call(
            env,
            &proxy,
            "upgrade",
            Vec::from_array(env, [new_wasm_hash.clone().into_val(env)]),
        ),
    }
}

fn invoke_runtime_call(
    env: &Env,
    proxy: &Address,
    fn_name: &str,
    args: Vec<Val>,
) -> Result<(), GovernanceError> {
    let fn_name = Symbol::new(env, fn_name);
    let auth_args = args.clone();
    env.authorize_as_current_contract(Vec::from_array(
        env,
        [InvokerContractAuthEntry::Contract(SubContractInvocation {
            context: ContractContext {
                contract: proxy.clone(),
                fn_name: fn_name.clone(),
                args: auth_args,
            },
            sub_invocations: Vec::new(env),
        })],
    ));
    let result: Result<Val, ContractError> = env.invoke_contract(proxy, &fn_name, args);
    result
        .map(|_| ())
        .map_err(|_| GovernanceError::RuntimeFailed)
}
