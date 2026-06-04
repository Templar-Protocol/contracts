//! The proposal "engine": authorization, validation, TTL computation, and the
//! dispatch table that translates a `GovernanceAction` into a runtime
//! sub-invocation (or a local effect like setting a TTL or toggling a role).

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    Address, Env, IntoVal, Symbol, Val, Vec,
};
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_soroban_common::ContractError;
use templar_proxy_oracle_soroban_governance_common::{
    GovernanceAction, GovernanceError, Role, MAX_PROPOSAL_TTL_NS,
};

use crate::{
    events::ActionTtlSet,
    roles,
    storage::{DataKey, KernelGovernance},
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
    if roles::has_role(env, caller, Role::Admin) || roles::has_role(env, caller, required) {
        Ok(())
    } else {
        Err(GovernanceError::Unauthorized)
    }
}

pub fn effective_ttl(
    governance: &KernelGovernance,
    action: &GovernanceAction,
    requested_ttl: u64,
) -> Result<u64, GovernanceError> {
    if requested_ttl > MAX_PROPOSAL_TTL_NS {
        return Err(GovernanceError::TtlExceedsMaximum);
    }
    let ttl = governance.effective_ttl(action, Nanoseconds::from_ns(requested_ttl));
    if ttl.as_ns() > MAX_PROPOSAL_TTL_NS {
        return Err(GovernanceError::TtlExceedsMaximum);
    }
    Ok(ttl.as_ns())
}

// One arm per `GovernanceAction` variant — the body is necessarily long.
#[allow(clippy::too_many_lines)]
pub fn execute_action(
    env: &Env,
    governance: &mut KernelGovernance,
    action: &GovernanceAction,
    caller: &Address,
) -> Result<(), GovernanceError> {
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
        GovernanceAction::SetManualTrip(asset, is_manually_tripped, metadata) => {
            invoke_runtime_call(
                env,
                &proxy,
                "set_manual_trip",
                Vec::from_array(
                    env,
                    [
                        asset.clone().into_val(env),
                        is_manually_tripped.into_val(env),
                        metadata.clone().into_val(env),
                    ],
                ),
            )
        }
        GovernanceAction::TransferOwnership(new_owner) => {
            // First leg of `stellar_access::ownable`'s two-step transfer.
            // The new owner must follow up with an `AcceptOwnership`
            // proposal (executed on whichever governance contract becomes
            // the new owner) or a direct `accept_ownership` call.
            // `live_until_ledger` is set to the maximum entry TTL window —
            // past that, the pending transfer expires and the proposal
            // must be re-submitted.
            let live_until_ledger = env.ledger().max_live_until_ledger();
            invoke_runtime_call(
                env,
                &proxy,
                "transfer_ownership",
                Vec::from_array(
                    env,
                    [
                        new_owner.clone().into_val(env),
                        live_until_ledger.into_val(env),
                    ],
                ),
            )
        }
        GovernanceAction::AcceptOwnership => {
            invoke_runtime_call(env, &proxy, "accept_ownership", Vec::new(env))
        }
        GovernanceAction::RenounceOwnership => {
            invoke_runtime_call(env, &proxy, "renounce_ownership", Vec::new(env))
        }
        GovernanceAction::SetActionTtl(kind, new_ttl_ns) => {
            governance
                .ttls
                .set(*kind, Nanoseconds::from_ns(*new_ttl_ns));
            ActionTtlSet {
                kind: *kind,
                new_ttl_ns: *new_ttl_ns,
            }
            .publish(env);
            Ok(())
        }
        GovernanceAction::SetRole(account, role, set) => {
            if *set {
                roles::grant(env, account, *role, caller);
                Ok(())
            } else {
                roles::revoke(env, account, *role, caller)
            }
        }
        GovernanceAction::Upgrade(new_wasm_hash) => invoke_runtime_call(
            env,
            &proxy,
            "upgrade",
            Vec::from_array(
                env,
                [
                    new_wasm_hash.clone().into_val(env),
                    env.current_contract_address().into_val(env),
                ],
            ),
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
