#![no_std]
// Every `#[contractimpl]` method in this crate is an ABI entry point and
// must take `env: Env` and `Address` by value.
#![allow(clippy::needless_pass_by_value)]

extern crate alloc;

use soroban_sdk::{contract, contractimpl, Address, Env, Vec};
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_governance_kernel::{
    CancelError as KernelCancelError, CreateError as KernelCreateError,
    ExecuteError as KernelExecuteError,
};
use templar_proxy_oracle_soroban_common::extend_instance_ttl;
use templar_proxy_oracle_soroban_governance_common::{
    GovernanceAction, GovernanceError, OperationKind, PendingProposal, Proposal, Role, TtlConfig,
    MAX_PROPOSAL_TTL_NS,
};

mod engine;
mod events;
mod roles;
mod storage;

pub use events::{
    ActionTtlSet, OwnershipTransferSubmitted, ProposalAccepted, ProposalRevoked, ProposalSubmitted,
    TtlExtended,
};

use engine::{effective_ttl, execute_action, now, require_authorized, validate_for_creator};
use storage::{
    load_header, load_proposal, proposal_from_kernel, proposal_to_kernel, remove_proposal,
    save_header, save_proposal, DataKey, KernelGovernance,
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
        initial_uniform_ttl_ns: u64,
    ) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        if env.storage().instance().has(&DataKey::ProxyOracle) {
            return Err(GovernanceError::AlreadyInitialized);
        }
        if initial_uniform_ttl_ns > MAX_PROPOSAL_TTL_NS {
            return Err(GovernanceError::TtlExceedsMaximum);
        }
        env.storage()
            .instance()
            .set(&DataKey::ProxyOracle, &proxy_oracle);
        save_header(
            &env,
            &KernelGovernance::new(
                TtlConfig::uniform(Nanoseconds::from_ns(initial_uniform_ttl_ns)),
                MAX_PENDING_PROPOSALS,
            ),
        );
        roles::grant(&env, &admin, Role::Admin, &admin);
        Ok(())
    }

    pub fn next_proposal_id(env: Env) -> Result<u64, GovernanceError> {
        extend_instance_ttl(&env);
        Ok(load_header(&env)?.next_id)
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
        effective_ttl(&load_header(&env)?, &operation, requested_ttl)
    }

    pub fn get_operation_ttl(env: Env, kind: OperationKind) -> Result<u64, GovernanceError> {
        extend_instance_ttl(&env);
        Ok(load_header(&env)?.ttls.get(kind).as_ns())
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
        let mut header = load_header(&env)?;
        let ttl_ns = effective_ttl(&header, &operation, requested_ttl)?;
        let kernel_proposal = header
            .create(
                id,
                operation.clone(),
                now(&env)?,
                caller,
                Nanoseconds::from_ns(ttl_ns),
            )
            .map_err(map_create_error)?;
        let proposal = proposal_from_kernel(kernel_proposal);
        save_proposal(&env, id, &proposal);
        save_header(&env, &header);
        ProposalSubmitted {
            id,
            valid_after_ns: proposal
                .created_at_ns
                .checked_add(proposal.ttl_ns)
                .ok_or(GovernanceError::ArithmeticOverflow)?,
            action_code: operation.action_code(),
        }
        .publish(&env);
        if let GovernanceAction::TransferOwnership(new_owner) = operation {
            OwnershipTransferSubmitted { id, new_owner }.publish(&env);
        }
        Ok(proposal)
    }

    pub fn cancel_proposal(env: Env, caller: Address, id: u64) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        let proposal = load_proposal(&env, id).ok_or(GovernanceError::ProposalNotFound)?;
        require_authorized(&env, &caller, &proposal.operation)?;
        let mut header = load_header(&env)?;
        header.cancel(id).map_err(map_cancel_error)?;
        remove_proposal(&env, id);
        save_header(&env, &header);
        ProposalRevoked { id }.publish(&env);
        Ok(())
    }

    pub fn execute_proposal(env: Env, caller: Address, id: u64) -> Result<(), GovernanceError> {
        extend_instance_ttl(&env);
        let proposal = load_proposal(&env, id).ok_or(GovernanceError::ProposalNotFound)?;
        require_authorized(&env, &caller, &proposal.operation)?;
        let mut header = load_header(&env)?;
        // Commit the authoritative state transition first: the kernel
        // re-validates the operation, enforces maturity, and drops the proposal
        // from the pending set. Only then do we fire side effects, so they can
        // never run for a proposal the kernel would reject.
        let kernel_proposal = proposal_to_kernel(proposal.clone());
        header
            .execute(id, &kernel_proposal, now(&env)?)
            .map_err(map_execute_error)?;
        execute_action(&env, &mut header, &proposal.operation, &caller)?;
        remove_proposal(&env, id);
        save_header(&env, &header);
        ProposalAccepted { id }.publish(&env);
        Ok(())
    }

    pub fn has_role(env: Env, account: Address, role: Role) -> bool {
        extend_instance_ttl(&env);
        roles::has_role(&env, &account, role)
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

    pub fn active_ids(env: Env) -> Vec<u64> {
        extend_instance_ttl(&env);
        let Ok(header) = load_header(&env) else {
            return Vec::new(&env);
        };
        let mut result = Vec::new(&env);
        for id in header.active_ids() {
            result.push_back(*id);
        }
        result
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
        if !roles::has_role(&env, &caller, Role::Admin) {
            return Err(GovernanceError::Unauthorized);
        }
        extend_instance_ttl(&env);
        TtlExtended {}.publish(&env);
        Ok(())
    }
}

fn map_create_error(error: KernelCreateError<GovernanceError>) -> GovernanceError {
    match error {
        KernelCreateError::IdOutOfOrder(_) => GovernanceError::ProposalOutOfOrder,
        KernelCreateError::IdOverflow => GovernanceError::ArithmeticOverflow,
        KernelCreateError::TooManyPendingProposals => GovernanceError::InvalidInput,
        KernelCreateError::Validation(error) => error,
    }
}

fn map_cancel_error(error: KernelCancelError) -> GovernanceError {
    match error {
        KernelCancelError::IdOutOfBounds(_) | KernelCancelError::ProposalDoesNotExist(_) => {
            GovernanceError::ProposalNotFound
        }
    }
}

fn map_execute_error(error: KernelExecuteError<GovernanceError>) -> GovernanceError {
    match error {
        KernelExecuteError::IdOutOfBounds(_) | KernelExecuteError::ProposalDoesNotExist(_) => {
            GovernanceError::ProposalNotFound
        }
        KernelExecuteError::TtlNotElapsed(_) => GovernanceError::ProposalNotMature,
        KernelExecuteError::Validation(error) => error,
    }
}

#[cfg(test)]
mod tests;
