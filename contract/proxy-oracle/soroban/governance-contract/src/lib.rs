#![no_std]
// Soroban contract entry points require `env: Env` and `Address` by value.
// Taking them by reference is not valid for the Soroban ABI and would not
// compile. The lint is suppressed at the file level because every public
// method in the #[contractimpl] block is an ABI entry point.
#![allow(clippy::needless_pass_by_value)]

extern crate alloc;

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contracterror, contractevent, contractimpl, contracttype, Address, Bytes, Env,
    IntoVal, Symbol, Val, Vec,
};
use templar_proxy_oracle_soroban_common::{
    extend_instance_ttl, Asset, CircuitBreakerConfig, CircuitBreakerUpdateConfig, ContractError,
    ProxyConfig, Role,
};

#[contract]
pub struct ProxyOracleGovernance;

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    ProxyOracle,
    ActionTtlNs,
    NextProposalId,
    PendingQueue,
}

#[contracttype]
#[derive(Clone)]
pub enum GovernanceAction {
    SetProxy(Asset, ProxyConfig),
    RemoveProxy(Asset),
    ConfigureBreakers(Asset, u64, u32),
    AddBreaker(Asset, CircuitBreakerConfig),
    RemoveBreaker(Asset, u32),
    UpdateBreaker(Asset, u32, CircuitBreakerUpdateConfig),
    SetManualTrip(Address, Asset, bool, Option<Bytes>),
    SetCircuitBreakerRole(Address, Role, bool),
    SetGovernance(Address),
    SetActionTtl(u64),
}

#[contracttype]
#[derive(Clone)]
pub struct PendingProposal {
    pub id: u64,
    pub action: GovernanceAction,
    pub valid_after_ns: u64,
}

#[contracttype]
#[derive(Clone)]
struct StoredPending {
    id: u64,
    action: GovernanceAction,
    valid_after_ns: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct ProposalSubmitted {
    #[topic]
    pub id: u64,
    pub valid_after_ns: u64,
    pub action_code: u32,
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

#[contractevent]
#[derive(Clone)]
pub struct GovernanceHandoffSubmitted {
    #[topic]
    pub id: u64,
    #[topic]
    pub new_governance: Address,
}

#[contractevent]
#[derive(Clone)]
pub struct ActionTtlSet {
    pub new_ttl_ns: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct TtlExtended {}

#[contracterror]
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub enum GovernanceError {
    AlreadyInitialized = 1,
    Unauthorized = 2,
    MissingConfig = 3,
    ProposalNotFound = 4,
    ProposalNotMature = 5,
    ArithmeticOverflow = 6,
    RuntimeFailed = 7,
    ProposalOutOfOrder = 8,
}

fn now_ns(env: &Env) -> Result<u64, GovernanceError> {
    env.ledger()
        .timestamp()
        .checked_mul(1_000_000_000)
        .ok_or(GovernanceError::ArithmeticOverflow)
}

fn load_address(env: &Env, key: DataKey) -> Result<Address, GovernanceError> {
    env.storage()
        .instance()
        .get(&key)
        .ok_or(GovernanceError::MissingConfig)
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), GovernanceError> {
    caller.require_auth();
    let admin = load_address(env, DataKey::Admin)?;
    if &admin != caller {
        return Err(GovernanceError::Unauthorized);
    }
    Ok(())
}

fn next_proposal_id(env: &Env) -> Result<u64, GovernanceError> {
    let current = env
        .storage()
        .instance()
        .get(&DataKey::NextProposalId)
        .unwrap_or(1_u64);
    let next = current
        .checked_add(1)
        .ok_or(GovernanceError::ArithmeticOverflow)?;
    env.storage()
        .instance()
        .set(&DataKey::NextProposalId, &next);
    Ok(current)
}

fn load_queue(env: &Env) -> Vec<StoredPending> {
    env.storage()
        .instance()
        .get(&DataKey::PendingQueue)
        .unwrap_or_else(|| Vec::new(env))
}

fn save_queue(env: &Env, queue: &Vec<StoredPending>) {
    env.storage().instance().set(&DataKey::PendingQueue, queue);
}

#[allow(clippy::too_many_lines)]
fn execute_action(env: &Env, action: &GovernanceAction) -> Result<(), GovernanceError> {
    let proxy = load_address(env, DataKey::ProxyOracle)?;
    match action {
        GovernanceAction::SetProxy(asset, config) => invoke_runtime_call(
            env,
            &proxy,
            "set_proxy",
            Vec::from_array(
                env,
                [
                    asset.clone().into_val(env),
                    Some(config.clone()).into_val(env),
                ],
            ),
        ),
        GovernanceAction::RemoveProxy(asset) => invoke_runtime_call(
            env,
            &proxy,
            "set_proxy",
            Vec::from_array(
                env,
                [
                    asset.clone().into_val(env),
                    Option::<ProxyConfig>::None.into_val(env),
                ],
            ),
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
        GovernanceAction::UpdateBreaker(asset, breaker_id, update) => invoke_runtime_call(
            env,
            &proxy,
            "update_breaker",
            Vec::from_array(
                env,
                [
                    asset.clone().into_val(env),
                    breaker_id.into_val(env),
                    update.clone().into_val(env),
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
        GovernanceAction::SetCircuitBreakerRole(account, role, is_granted) => invoke_runtime_call(
            env,
            &proxy,
            "set_circuit_breaker_role",
            Vec::from_array(
                env,
                [
                    account.clone().into_val(env),
                    role.clone().into_val(env),
                    is_granted.into_val(env),
                ],
            ),
        ),
        GovernanceAction::SetGovernance(governance) => invoke_runtime_call(
            env,
            &proxy,
            "set_governance",
            Vec::from_array(env, [governance.clone().into_val(env)]),
        ),
        GovernanceAction::SetActionTtl(new_ttl_ns) => {
            env.storage()
                .instance()
                .set(&DataKey::ActionTtlNs, new_ttl_ns);
            ActionTtlSet {
                new_ttl_ns: *new_ttl_ns,
            }
            .publish(env);
            Ok(())
        }
    }
}

fn action_code(action: &GovernanceAction) -> u32 {
    match action {
        GovernanceAction::SetProxy(_, _) => 1,
        GovernanceAction::RemoveProxy(_) => 2,
        GovernanceAction::ConfigureBreakers(_, _, _) => 3,
        GovernanceAction::AddBreaker(_, _) => 4,
        GovernanceAction::RemoveBreaker(_, _) => 5,
        GovernanceAction::UpdateBreaker(_, _, _) => 6,
        GovernanceAction::SetManualTrip(_, _, _, _) => 7,
        GovernanceAction::SetCircuitBreakerRole(_, _, _) => 8,
        GovernanceAction::SetGovernance(_) => 9,
        GovernanceAction::SetActionTtl(_) => 10,
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

fn lowest_pending_id(queue: &Vec<StoredPending>) -> Option<u64> {
    queue.iter().map(|proposal| proposal.id).min()
}

#[contractimpl]
impl ProxyOracleGovernance {
    pub fn __constructor(
        env: Env,
        admin: Address,
        proxy_oracle: Address,
        action_ttl_ns: u64,
    ) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(GovernanceError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::ProxyOracle, &proxy_oracle);
        env.storage()
            .instance()
            .set(&DataKey::ActionTtlNs, &action_ttl_ns);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &1_u64);
        env.storage()
            .instance()
            .set(&DataKey::PendingQueue, &Vec::<StoredPending>::new(&env));
        Ok(())
    }

    pub fn submit(
        env: Env,
        caller: Address,
        action: GovernanceAction,
    ) -> Result<u64, GovernanceError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        let id = next_proposal_id(&env)?;
        let now = now_ns(&env)?;
        let ttl: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ActionTtlNs)
            .ok_or(GovernanceError::MissingConfig)?;
        let valid_after_ns = now
            .checked_add(ttl)
            .ok_or(GovernanceError::ArithmeticOverflow)?;
        let mut queue = load_queue(&env);
        queue.push_back(StoredPending {
            id,
            action: action.clone(),
            valid_after_ns,
        });
        save_queue(&env, &queue);
        ProposalSubmitted {
            id,
            valid_after_ns,
            action_code: action_code(&action),
        }
        .publish(&env);
        if let GovernanceAction::SetGovernance(new_governance) = action {
            GovernanceHandoffSubmitted { id, new_governance }.publish(&env);
        }
        Ok(id)
    }

    pub fn accept(env: Env, caller: Address, proposal_id: u64) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        let now_ns = now_ns(&env)?;
        let mut queue = load_queue(&env);
        let Some(lowest_id) = lowest_pending_id(&queue) else {
            return Err(GovernanceError::ProposalNotFound);
        };
        if proposal_id != lowest_id {
            return Err(GovernanceError::ProposalOutOfOrder);
        }
        let index = queue
            .iter()
            .position(|proposal| proposal.id == proposal_id)
            .and_then(|i| u32::try_from(i).ok())
            .ok_or(GovernanceError::ProposalNotFound)?;
        let proposal = queue.get(index).ok_or(GovernanceError::ProposalNotFound)?;
        if proposal.valid_after_ns > now_ns {
            return Err(GovernanceError::ProposalNotMature);
        }
        let proposal = queue.get(index).ok_or(GovernanceError::ProposalNotFound)?;
        queue.remove(index);
        execute_action(&env, &proposal.action)?;
        save_queue(&env, &queue);
        ProposalAccepted { id: proposal.id }.publish(&env);
        Ok(())
    }

    pub fn revoke(env: Env, caller: Address, proposal_id: u64) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        let mut queue = load_queue(&env);
        let index = queue
            .iter()
            .position(|proposal| proposal.id == proposal_id)
            .and_then(|i| u32::try_from(i).ok())
            .ok_or(GovernanceError::ProposalNotFound)?;
        let removed = queue.get(index).ok_or(GovernanceError::ProposalNotFound)?;
        queue.remove(index);
        save_queue(&env, &queue);
        ProposalRevoked { id: removed.id }.publish(&env);
        Ok(())
    }

    pub fn pending(env: Env, proposal_id: u64) -> Result<PendingProposal, GovernanceError> {
        extend_instance_ttl(&env);
        for proposal in load_queue(&env).iter() {
            if proposal.id == proposal_id {
                return Ok(PendingProposal {
                    id: proposal.id,
                    action: proposal.action,
                    valid_after_ns: proposal.valid_after_ns,
                });
            }
        }
        Err(GovernanceError::ProposalNotFound)
    }

    pub fn pending_ids(env: Env) -> Vec<u64> {
        extend_instance_ttl(&env);
        let mut ids = Vec::new(&env);
        for proposal in load_queue(&env).iter() {
            ids.push_back(proposal.id);
        }
        ids
    }

    pub fn action_ttl_ns(env: Env) -> Result<u64, GovernanceError> {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::ActionTtlNs)
            .ok_or(GovernanceError::MissingConfig)
    }

    pub fn admin(env: Env) -> Result<Address, GovernanceError> {
        extend_instance_ttl(&env);
        load_address(&env, DataKey::Admin)
    }

    pub fn proxy_oracle(env: Env) -> Result<Address, GovernanceError> {
        extend_instance_ttl(&env);
        load_address(&env, DataKey::ProxyOracle)
    }

    pub fn extend_ttl(env: Env, caller: Address) -> Result<(), GovernanceError> {
        require_admin(&env, &caller)?;
        extend_instance_ttl(&env);
        TtlExtended {}.publish(&env);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
