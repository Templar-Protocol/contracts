use std::ops::{Deref, DerefMut};

use near_sdk::{
    borsh::{BorshDeserialize, BorshSerialize},
    env, near, require, AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise,
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
    cancellation_event_size, gen_ext_governance, policy_recovery_working_set_bytes,
    proposal_body_size, CreateProposalArgs, Event, GovernancePolicy, Operation, Proposal,
    ReflexiveOperation, Role, ADMIN_RESERVED_CANCELLATION_LOG_BYTES, ADMIN_RESERVED_PROPOSAL_BYTES,
    MAX_GOVERNANCE_WORKING_SET_BYTES, MAX_PENDING_CANCELLATION_LOG_BYTES, MAX_PENDING_PROPOSALS,
    MAX_PROPOSAL_TTL,
};

mod state;
use state::State;

gen_ext_governance!(ext_proxy_governance, ProxyGovernanceInterface, Operation);

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
pub(crate) enum StorageKey {
    Proposals,
    PendingProposalBytes,
}

#[derive(BorshDeserialize, BorshSerialize, Default)]
#[borsh(crate = "near_sdk::borsh")]
struct PendingProposalUsage {
    body_bytes: u64,
    cancellation_log_bytes: u64,
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

    fn is_authorized_for(&self, account_id: &AccountId, operation: &Operation) -> bool {
        <Self as Rbac>::has_role(account_id, &Role::Admin)
            || <Self as Rbac>::has_role(account_id, &operation.required_role(self.header.ttls()))
    }

    fn assert_authorized(&self, operation: &Operation) {
        require!(
            self.is_authorized_for(&env::predecessor_account_id(), operation),
            "Caller is not authorized for this operation"
        );
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

    fn pending_usage() -> Option<PendingProposalUsage> {
        let key = near_sdk::borsh::to_vec(&StorageKey::PendingProposalBytes)
            .expect_or_reject("serialize storage key");
        env::storage_read(&key).map(|bytes| {
            let usage: PendingProposalUsage = near_sdk::borsh::from_slice(&bytes)
                .unwrap_or_else(|_| env::panic_str("Corrupt pending proposal accounting"));
            require!(
                usage.body_bytes <= MAX_GOVERNANCE_WORKING_SET_BYTES
                    && usage.cancellation_log_bytes <= MAX_PENDING_CANCELLATION_LOG_BYTES,
                "Corrupt pending proposal accounting"
            );
            usage
        })
    }

    fn set_pending_usage(usage: &PendingProposalUsage) {
        let key = near_sdk::borsh::to_vec(&StorageKey::PendingProposalBytes)
            .expect_or_reject("serialize storage key");
        let value =
            near_sdk::borsh::to_vec(usage).expect_or_reject("serialize proposal accounting");
        env::storage_write(&key, &value);
    }

    fn remove_pending_usage(id: u32, proposal: &Proposal<Operation>) {
        if let Some(mut usage) = Self::pending_usage() {
            usage.body_bytes = usage
                .body_bytes
                .checked_sub(
                    proposal_body_size(&proposal.operation, &proposal.created_by)
                        .expect_or_reject("measure proposal body"),
                )
                .unwrap_or_else(|| env::panic_str("Corrupt pending proposal accounting"));
            usage.cancellation_log_bytes = usage
                .cancellation_log_bytes
                .checked_sub(
                    cancellation_event_size(id, &proposal.operation)
                        .expect_or_reject("measure cancellation event"),
                )
                .unwrap_or_else(|| env::panic_str("Corrupt pending proposal accounting"));
            Self::set_pending_usage(&usage);
        }
    }

    fn state_size(&self) -> u64 {
        u64::try_from(
            near_sdk::borsh::object_length(self).expect_or_reject("serialize governance state"),
        )
        .expect_or_reject("governance state size exceeds u64")
    }

    fn assert_working_set(&self) {
        if let Some(pending) = Self::pending_usage() {
            require!(
                self.state_size()
                    .checked_add(pending.body_bytes)
                    .is_some_and(|size| size <= MAX_GOVERNANCE_WORKING_SET_BYTES),
                "Governance working set byte limit exceeded"
            );
        }
        self.assert_policy_recovery();
    }

    fn assert_policy_recovery(&self) {
        require!(
            policy_recovery_working_set_bytes(self.state_size(), self.header.ttls())
                .is_some_and(|size| size <= MAX_GOVERNANCE_WORKING_SET_BYTES),
            "Governance policy exceeds recoverable working set"
        );
    }

    fn cancel_proposal_inner(&mut self, id: u32) {
        // Load the body before changing the ledger: a missing/corrupt body is an error.
        let proposal = self.proposals.get(&id).unwrap_or_reject();
        let kind = proposal.operation.kind();
        let method = proposal.operation.method();
        Self::remove_pending_usage(id, proposal);
        self.header.cancel(u64::from(id)).unwrap_or_reject();
        self.proposals.remove(&id).unwrap_or_reject();
        Event::Cancelled { id, kind, method }.emit();
    }

    fn purge_unauthorized_proposals(&mut self, account_id: &AccountId) {
        let mut index = 0;
        // Cancellation shifts the next active ID into the current position. The ledger is capped
        // at 64 entries; no second index or copy of the proposal bodies is needed.
        while let Some(&id) = self.header.active_ids().get(index) {
            let id = Self::id_to_u32(id);
            let proposal = self.proposals.get(&id).unwrap_or_reject();
            if &proposal.created_by == account_id
                && !self.is_authorized_for(account_id, &proposal.operation)
            {
                self.cancel_proposal_inner(id);
            } else {
                index += 1;
            }
        }
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
        if !<Self as Rbac>::has_role(&env::predecessor_account_id(), &Role::Admin)
            && self.header.active_ids().len()
                >= usize::try_from(self.header.max_pending_proposals().saturating_sub(1))
                    .unwrap_or(usize::MAX)
        {
            operation.validate_on_create().unwrap_or_reject();
            env::panic_str("too many pending proposals");
        }
        let creator = env::predecessor_account_id();
        let mut usage = Self::pending_usage().unwrap_or_else(|| {
            require!(
                self.header.active_ids().is_empty(),
                "Drain legacy proposals before activating pending byte accounting"
            );
            PendingProposalUsage::default()
        });
        self.assert_policy_recovery();
        let size =
            proposal_body_size(&operation, &creator).expect_or_reject("measure proposal body");
        let is_admin = <Self as Rbac>::has_role(&creator, &Role::Admin);
        let limit = if is_admin {
            MAX_GOVERNANCE_WORKING_SET_BYTES
        } else {
            MAX_GOVERNANCE_WORKING_SET_BYTES - ADMIN_RESERVED_PROPOSAL_BYTES
        };
        usage.body_bytes = usage
            .body_bytes
            .checked_add(size)
            .filter(|&value| {
                self.state_size()
                    .checked_add(8)
                    .and_then(|state| state.checked_add(value))
                    .is_some_and(|working_set| working_set <= limit)
            })
            .unwrap_or_else(|| env::panic_str("Governance working set byte limit exceeded"));
        let log_limit = if is_admin {
            MAX_PENDING_CANCELLATION_LOG_BYTES
        } else {
            MAX_PENDING_CANCELLATION_LOG_BYTES - ADMIN_RESERVED_CANCELLATION_LOG_BYTES
        };
        usage.cancellation_log_bytes = usage
            .cancellation_log_bytes
            .checked_add(
                cancellation_event_size(id, &operation)
                    .expect_or_reject("measure cancellation event"),
            )
            .filter(|&value| value <= log_limit)
            .unwrap_or_else(|| env::panic_str("Pending cancellation log byte limit exceeded"));
        let proposal = self
            .header
            .create(
                u64::from(id),
                operation,
                Nanoseconds::near_timestamp(),
                creator,
                requested_ttl,
            )
            .unwrap_or_reject();
        if proposal.ttl > MAX_PROPOSAL_TTL {
            env::panic_str("Proposal TTL exceeds maximum allowed");
        }
        let kind = proposal.operation.kind();
        let method = proposal.operation.method();
        Self::set_pending_usage(&usage);
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
        let proposal = self.proposals.get(&id).unwrap_or_reject();
        let caller = env::predecessor_account_id();
        require!(
            proposal.created_by == caller || <Self as Rbac>::has_role(&caller, &Role::Admin),
            "Only the proposal creator or an admin can cancel"
        );
        self.cancel_proposal_inner(id);
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
        if let Operation::Reflexive(ReflexiveOperation::SetRole {
            account_id,
            role,
            set: false,
        }) = &proposal.operation
        {
            if <Self as Rbac>::has_role(account_id, role)
                && self.header.active_ids().len() > 1
                && Self::pending_usage().is_none()
            {
                env::panic_str("Drain legacy proposals before revoking roles");
            }
        }

        // Commit the authoritative transition first (validates, enforces
        // maturity, drops the id from the pending set), then fire effects.
        self.header
            .execute(u64::from(id), &proposal, Nanoseconds::near_timestamp())
            .unwrap_or_reject();
        Self::remove_pending_usage(id, &proposal);
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
                    } else if <Self as Rbac>::has_role(&account_id, &role) {
                        <Self as Rbac>::remove_role(self, &account_id, &role);
                        self.purge_unauthorized_proposals(&account_id);
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
        self.assert_working_set();

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
        self_.assert_policy_recovery();

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
