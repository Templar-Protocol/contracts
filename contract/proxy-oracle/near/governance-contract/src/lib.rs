use std::ops::{Deref, DerefMut};

use near_sdk::{
    borsh::BorshSerialize, env, near, require, AccountId, BorshStorageKey, Gas, NearToken,
    PanicOnDefault, Promise,
};
use near_sdk_contract_tools::{rbac::Rbac, Rbac};
use templar_common::{
    contract::list,
    upgrade::MIGRATE_METHOD,
    versioned_state::{impl_versioned_state, StateVersion, VersionedState},
    Nanoseconds, UnwrapReject,
};
use templar_proxy_oracle_governance_kernel::OperationPolicy;
use templar_proxy_oracle_near_governance_common::{
    gen_ext_governance, CreateProposalArgs, Event, GovernancePolicy, Operation, Proposal,
    ReflexiveOperation, Role, MAX_PENDING_PROPOSALS, MAX_PROPOSAL_TTL,
};

mod state;
use state::State;

gen_ext_governance!(ext_proxy_governance, ProxyGovernanceInterface, Operation);

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
pub(crate) enum StorageKey {
    Proposals,
}

/// Wraps the versioned governance `State` (kernel ledger header, proposal bodies, governed oracle
/// account). Role membership lives in `near-sdk-contract-tools` RBAC storage, separate from `state`.
#[derive(Debug, Rbac, PanicOnDefault)]
#[rbac(roles = "Role")]
#[near(contract_state)]
pub struct Contract {
    pub state: VersionedState<State>,
}

impl Deref for Contract {
    type Target = State;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for Contract {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl_versioned_state!(Contract, State, state::migration::Migration);

impl Contract {
    /// Gas for the `migrate` call batched onto a `SelfUpgrade` self-deploy of this contract.
    pub const GAS_FOR_MIGRATE: Gas = Gas::from_tgas(250);

    fn assert_authorized(&self, operation: &Operation) {
        let required = operation.required_role(self.header.ttls());
        let caller = env::predecessor_account_id();
        let has_role = <Self as Rbac>::has_role(&caller, &Role::Admin)
            || <Self as Rbac>::has_role(&caller, &required);
        require!(has_role, "Caller is not authorized for this operation");
    }

    fn assert_can_set_role(account_id: &AccountId, role: Role, set: bool) {
        let removes_admin =
            !set && role == Role::Admin && <Self as Rbac>::has_role(account_id, &Role::Admin);
        require!(
            !removes_admin
                || <Self as Rbac>::with_members_of(&Role::Admin, |members| members.len()) > 1,
            "Cannot remove the last admin"
        );
    }

    fn id_to_u32(id: u64) -> u32 {
        u32::try_from(id).unwrap_or_else(|_| env::panic_str("Proposal ID exceeds u32"))
    }

    /// Returns a borrow so the borsh entrypoint never copies the payload it exists to carry cheaply.
    fn create_proposal_inner(
        &mut self,
        id: u32,
        operation: Operation,
        requested_ttl: Nanoseconds,
    ) -> &Proposal<Operation> {
        near_sdk::assert_one_yocto();
        self.assert_authorized(&operation);

        if requested_ttl > MAX_PROPOSAL_TTL {
            env::panic_str("Proposal TTL exceeds maximum allowed");
        }
        let proposal = self
            .header
            .create(
                u64::from(id),
                operation,
                Nanoseconds::near_timestamp(),
                env::predecessor_account_id(),
                requested_ttl,
            )
            .unwrap_or_reject();
        if proposal.ttl > MAX_PROPOSAL_TTL {
            env::panic_str("Proposal TTL exceeds maximum allowed");
        }
        let kind = proposal.operation.kind();
        let method = proposal.operation.method();
        self.proposals.insert(id, proposal);
        Event::Created { id, kind, method }.emit();
        &self.proposals[&id]
    }
}

#[near]
impl ProxyGovernanceInterface for Contract {
    fn next_proposal_id(&self) -> u32 {
        Self::id_to_u32(self.header.next_id())
    }

    fn proposal_count(&self) -> u32 {
        u32::try_from(self.header.active_ids().len()).unwrap_or(u32::MAX)
    }

    fn list_proposals(&self, offset: Option<u32>, count: Option<u32>) -> Vec<u32> {
        list(
            self.header
                .active_ids()
                .iter()
                .copied()
                .map(Self::id_to_u32),
            offset,
            count,
        )
    }

    fn get_proposal(&self, id: u32) -> Option<Proposal<Operation>> {
        self.proposals.get(&id).cloned()
    }

    fn get_effective_proposal_ttl(
        &self,
        operation: Operation,
        requested_ttl: Nanoseconds,
    ) -> Nanoseconds {
        operation.minimum_ttl(self.header.ttls()).max(requested_ttl)
    }

    fn get_governance_policy(&self) -> GovernancePolicy {
        self.header.ttls().clone()
    }

    #[payable]
    fn create_proposal(
        &mut self,
        id: u32,
        operation: Operation,
        requested_ttl: Nanoseconds,
    ) -> Proposal<Operation> {
        self.create_proposal_inner(id, operation, requested_ttl)
            .clone()
    }

    #[payable]
    fn create_proposal_borsh(&mut self, #[serializer(borsh)] args: CreateProposalArgs<Operation>) {
        self.create_proposal_inner(args.id, args.operation, args.requested_ttl);
    }

    #[payable]
    fn cancel_proposal(&mut self, id: u32) {
        near_sdk::assert_one_yocto();
        let operation = self.proposals.get(&id).unwrap_or_reject().operation.clone();
        self.assert_authorized(&operation);

        self.header.cancel(u64::from(id)).unwrap_or_reject();
        let proposal = self.proposals.remove(&id).unwrap_or_reject();
        Event::Cancelled {
            id,
            kind: proposal.operation.kind(),
            method: proposal.operation.method(),
        }
        .emit();
    }

    #[payable]
    fn execute_proposal(&mut self, id: u32) {
        near_sdk::assert_one_yocto();

        let proposal = self.proposals.get(&id).unwrap_or_reject().clone();
        self.assert_authorized(&proposal.operation);
        if let Operation::Reflexive(ReflexiveOperation::SetRole {
            account_id,
            role,
            set,
        }) = &proposal.operation
        {
            Self::assert_can_set_role(account_id, *role, *set);
        }

        // Commit the authoritative transition first (validates, enforces
        // maturity, drops the id from the pending set), then fire effects.
        self.header
            .execute(u64::from(id), &proposal, Nanoseconds::near_timestamp())
            .unwrap_or_reject();
        let operation = self.proposals.remove(&id).unwrap_or_reject().operation;
        let kind = operation.kind();
        let method = operation.method();

        let proxy_oracle_id = self.proxy_oracle_id.clone();

        match operation {
            Operation::Reflexive(reflexive) => match reflexive {
                ReflexiveOperation::SetReflexiveTtl { kind, ttl } => {
                    self.header
                        .ttls_mut()
                        .set_reflexive_ttl(kind, ttl)
                        .unwrap_or_reject();
                }
                ReflexiveOperation::SetTargetDefault { policy } => {
                    self.header
                        .ttls_mut()
                        .set_target_default(policy)
                        .unwrap_or_reject();
                }
                ReflexiveOperation::SetMethodPolicy { method, policy } => {
                    self.header
                        .ttls_mut()
                        .set_method_policy(method, policy)
                        .unwrap_or_reject();
                }
                ReflexiveOperation::SetRole {
                    account_id,
                    role,
                    set,
                } => {
                    Self::assert_can_set_role(&account_id, role, set);
                    if set {
                        <Self as Rbac>::add_role(self, &account_id, &role);
                    } else {
                        <Self as Rbac>::remove_role(self, &account_id, &role);
                    }
                }
                ReflexiveOperation::SelfUpgrade { code, migrate_args } => {
                    code.deploy_and_migrate(MIGRATE_METHOD, migrate_args, Self::GAS_FOR_MIGRATE)
                        .detach();
                }
            },
            Operation::TargetFunctionCall(call) => {
                Promise::new(proxy_oracle_id)
                    .function_call(
                        call.method_name,
                        call.args.0,
                        NearToken::from_yoctonear(call.attached_deposit.0),
                        call.gas,
                    )
                    .detach();
            }
        }

        // Emitted last: the reflexive mutations above are fallible, and a failed one reverts state
        // while leaving its logs on the receipt. Emitting first would advertise an execution that
        // did not happen.
        Event::Executed { id, kind, method }.emit();
    }
}

#[near]
#[allow(clippy::needless_pass_by_value)]
impl Contract {
    #[init]
    pub fn new(proxy_oracle_id: AccountId, admin_id: AccountId, policy: GovernancePolicy) -> Self {
        // `policy` deserialized through `GovernancePolicyWire`, so it is already within bounds —
        // an out-of-range init policy is rejected while parsing the args, before reaching here.
        let mut self_ = Self {
            state: State::new((proxy_oracle_id, policy)),
        };

        <Self as Rbac>::add_role(&mut self_, &admin_id, &Role::Admin);

        self_
    }

    pub fn get_proxy_oracle_id(&self) -> &AccountId {
        &self.proxy_oracle_id
    }

    pub fn has_role(&self, account_id: AccountId, role: Role) -> bool {
        <Self as Rbac>::has_role(&account_id, &role)
    }

    pub fn list_role(&self, role: Role, offset: Option<u32>, count: Option<u32>) -> Vec<AccountId> {
        list(<Self as Rbac>::iter_members_of(&role), offset, count)
    }

    pub fn get_roles(&self, account_id: AccountId) -> Vec<Role> {
        Role::ALL
            .into_iter()
            .filter(|role| <Self as Rbac>::has_role(&account_id, role))
            .collect()
    }
}

#[cfg(test)]
mod tests;
