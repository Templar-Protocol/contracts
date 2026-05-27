#![no_std]
// Every `#[contractimpl]` method in this crate is an ABI entry point and
// must take `env: Env` and `Address` by value.
#![allow(clippy::needless_pass_by_value)]

extern crate alloc;

use soroban_sdk::{contract, contractimpl, Address, Env, Vec};
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_soroban_common::{
    extend_instance_ttl, GovernanceAction, GovernanceError, OperationKind, PendingProposal,
    Proposal, Role, TtlConfig, MAX_PROPOSAL_TTL_NS,
};

mod engine;
mod events;
mod roles;
mod storage;

pub use events::{
    ActionTtlSet, GovernanceHandoffSubmitted, ProposalAccepted, ProposalRevoked, ProposalSubmitted,
    TtlExtended,
};

use engine::{effective_ttl, execute_action, now, require_authorized, validate_for_creator};
use storage::{
    load_proposal, load_proposal_count, load_proposal_ids, load_ttls, remove_proposal,
    remove_proposal_id, save_proposal, save_proposal_count, save_proposal_ids, save_ttls, DataKey,
};

const MAX_PENDING_PROPOSALS: u32 = 64;

#[contract]
pub struct ProxyOracleGovernance;

#[contractimpl]
impl ProxyOracleGovernance {
    pub fn __constructor(
        env: Env,
        admin: Address,
        proxy_oracle: Address,
        action_ttl_ns: u64,
    ) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        if env.storage().instance().has(&DataKey::ProxyOracle) {
            return Err(GovernanceError::AlreadyInitialized);
        }
        if action_ttl_ns > MAX_PROPOSAL_TTL_NS {
            return Err(GovernanceError::TtlExceedsMaximum);
        }
        env.storage()
            .instance()
            .set(&DataKey::ProxyOracle, &proxy_oracle);
        save_ttls(
            &env,
            &TtlConfig::uniform(Nanoseconds::from_ns(action_ttl_ns)),
        );
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &0_u64);
        save_proposal_ids(&env, &Vec::<u64>::new(&env));
        save_proposal_count(&env, 0);
        roles::grant(&env, &admin, Role::Admin);
        Ok(())
    }

    pub fn next_proposal_id(env: Env) -> Result<u64, GovernanceError> {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .ok_or(GovernanceError::MissingConfig)
    }

    pub fn proposal_count(env: Env) -> u32 {
        extend_instance_ttl(&env);
        load_proposal_count(&env)
    }

    pub fn list_proposals(env: Env, offset: u32, count: u32) -> Vec<u64> {
        extend_instance_ttl(&env);
        let ids = load_proposal_ids(&env);
        let mut result = Vec::new(&env);
        let end = offset.saturating_add(count);
        for i in offset..end {
            if let Some(id) = ids.get(i) {
                result.push_back(id);
            }
        }
        result
    }

    pub fn get_proposal(env: Env, id: u64) -> Option<Proposal> {
        extend_instance_ttl(&env);
        load_proposal(&env, id)
    }

    pub fn get_effective_proposal_ttl(
        env: Env,
        operation: GovernanceAction,
        requested_ttl: u64,
    ) -> Result<u64, GovernanceError> {
        extend_instance_ttl(&env);
        effective_ttl(&load_ttls(&env)?, &operation, requested_ttl)
    }

    pub fn get_operation_ttl(env: Env, kind: OperationKind) -> Result<u64, GovernanceError> {
        extend_instance_ttl(&env);
        Ok(load_ttls(&env)?.get(kind).as_ns())
    }

    pub fn create_proposal(
        env: Env,
        caller: Address,
        id: u64,
        operation: GovernanceAction,
        requested_ttl: u64,
    ) -> Result<Proposal, GovernanceError> {
        extend_instance_ttl(&env);
        require_authorized(&env, &caller, &operation)?;
        validate_for_creator(&caller, &operation)?;
        let next_id = env
            .storage()
            .instance()
            .get::<_, u64>(&DataKey::NextProposalId)
            .ok_or(GovernanceError::MissingConfig)?;
        if id != next_id {
            return Err(GovernanceError::ProposalOutOfOrder);
        }
        let ttl_ns = effective_ttl(&load_ttls(&env)?, &operation, requested_ttl)?;
        if load_proposal_count(&env) >= MAX_PENDING_PROPOSALS {
            return Err(GovernanceError::InvalidInput);
        }
        let proposal = Proposal {
            operation: operation.clone(),
            created_at_ns: now(&env)?.as_ns(),
            ttl_ns,
            created_by: caller,
        };
        save_proposal(&env, id, &proposal);
        let mut ids = load_proposal_ids(&env);
        ids.push_back(id);
        save_proposal_ids(&env, &ids);
        save_proposal_count(&env, load_proposal_count(&env) + 1);
        env.storage().instance().set(
            &DataKey::NextProposalId,
            &id.checked_add(1)
                .ok_or(GovernanceError::ArithmeticOverflow)?,
        );
        ProposalSubmitted {
            id,
            valid_after_ns: proposal
                .created_at_ns
                .checked_add(proposal.ttl_ns)
                .ok_or(GovernanceError::ArithmeticOverflow)?,
            action_code: operation.action_code(),
        }
        .publish(&env);
        if let GovernanceAction::SetGovernance(new_governance) = operation {
            GovernanceHandoffSubmitted { id, new_governance }.publish(&env);
        }
        Ok(proposal)
    }

    pub fn cancel_proposal(env: Env, caller: Address, id: u64) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        let proposal = load_proposal(&env, id).ok_or(GovernanceError::ProposalNotFound)?;
        require_authorized(&env, &caller, &proposal.operation)?;
        remove_proposal(&env, id);
        remove_proposal_id(&env, id);
        save_proposal_count(&env, load_proposal_count(&env).saturating_sub(1));
        ProposalRevoked { id }.publish(&env);
        Ok(())
    }

    pub fn execute_proposal(env: Env, caller: Address, id: u64) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        let proposal = load_proposal(&env, id).ok_or(GovernanceError::ProposalNotFound)?;
        require_authorized(&env, &caller, &proposal.operation)?;
        let elapsed = now(&env)?.saturating_sub(Nanoseconds::from_ns(proposal.created_at_ns));
        if elapsed < Nanoseconds::from_ns(proposal.ttl_ns) {
            return Err(GovernanceError::ProposalNotMature);
        }
        execute_action(&env, &proposal.operation)?;
        remove_proposal(&env, id);
        remove_proposal_id(&env, id);
        save_proposal_count(&env, load_proposal_count(&env).saturating_sub(1));
        ProposalAccepted { id }.publish(&env);
        Ok(())
    }

    pub fn has_role(env: Env, account: Address, role: Role) -> bool {
        extend_instance_ttl(&env);
        roles::has(&env, &account, role)
    }

    pub fn list_role(env: Env, role: Role) -> Vec<Address> {
        extend_instance_ttl(&env);
        roles::members(&env, role)
    }

    pub fn get_roles(env: Env, account: Address) -> Vec<Role> {
        extend_instance_ttl(&env);
        roles::roles_of(&env, &account)
    }

    pub fn submit(
        env: Env,
        caller: Address,
        action: GovernanceAction,
    ) -> Result<u64, GovernanceError> {
        let id = Self::next_proposal_id(env.clone())?;
        Self::create_proposal(env, caller, id, action, 0).map(|_| id)
    }

    pub fn accept(env: Env, caller: Address, proposal_id: u64) -> Result<(), GovernanceError> {
        Self::execute_proposal(env, caller, proposal_id)
    }

    pub fn revoke(env: Env, caller: Address, proposal_id: u64) -> Result<(), GovernanceError> {
        Self::cancel_proposal(env, caller, proposal_id)
    }

    pub fn pending(env: Env, proposal_id: u64) -> Result<PendingProposal, GovernanceError> {
        extend_instance_ttl(&env);
        let proposal = Self::get_proposal(env.clone(), proposal_id)
            .ok_or(GovernanceError::ProposalNotFound)?;
        Ok(PendingProposal {
            id: proposal_id,
            action: proposal.operation,
            valid_after_ns: proposal
                .created_at_ns
                .checked_add(proposal.ttl_ns)
                .ok_or(GovernanceError::ArithmeticOverflow)?,
        })
    }

    pub fn pending_ids(env: Env) -> Vec<u64> {
        let count = Self::proposal_count(env.clone());
        Self::list_proposals(env, 0, count)
    }

    pub fn action_ttl_ns(env: Env) -> Result<u64, GovernanceError> {
        Self::get_operation_ttl(env, OperationKind::SetProxy)
    }

    pub fn admin(env: Env) -> Result<Address, GovernanceError> {
        extend_instance_ttl(&env);
        roles::members(&env, Role::Admin)
            .get(0)
            .ok_or(GovernanceError::MissingConfig)
    }

    pub fn proxy_oracle(env: Env) -> Result<Address, GovernanceError> {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::ProxyOracle)
            .ok_or(GovernanceError::MissingConfig)
    }

    pub fn extend_ttl(env: Env, caller: Address) -> Result<(), GovernanceError> {
        caller.require_auth();
        if !roles::has(&env, &caller, Role::Admin) {
            return Err(GovernanceError::Unauthorized);
        }
        extend_instance_ttl(&env);
        TtlExtended {}.publish(&env);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
