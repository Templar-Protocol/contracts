use near_sdk::{
    borsh::BorshSerialize, env, near, require, store::LookupMap, AccountId, BorshStorageKey, Gas,
    NearToken, PanicOnDefault, Promise,
};
use near_sdk_contract_tools::{rbac::Rbac, Rbac};
use templar_common::{contract::list, Nanoseconds, UnwrapReject};
use templar_proxy_oracle_near_common::governance::ext_proxy_oracle_admin;
use templar_proxy_oracle_near_governance_common::{
    gen_ext_governance, Event, Governance, Operation, OperationKind, Proposal, Role, TtlConfig,
    MAX_PROPOSAL_TTL,
};

gen_ext_governance!(ext_proxy_governance, ProxyGovernanceInterface, Operation);

const MAX_PENDING_PROPOSALS: u32 = 64;

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Proposals,
}

#[derive(Debug, Rbac, PanicOnDefault)]
#[rbac(roles = "Role")]
#[near(contract_state)]
pub struct Contract {
    /// The governance kernel ledger: `next_id`, pending id set, TTL table, and
    /// the pending cap. Proposal bodies are stored per-key in `proposals`;
    /// role membership lives in `near-sdk-contract-tools` RBAC storage.
    pub header: Governance,
    pub proposals: LookupMap<u32, Proposal<Operation>>,
    pub proxy_oracle_id: AccountId,
}

impl Contract {
    pub const GAS_FOR_ADMIN_UPGRADE: Gas = Gas::from_tgas(280);

    fn assert_authorized(operation: &Operation) {
        let required = operation.required_role();
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
}

#[near]
impl ProxyGovernanceInterface for Contract {
    fn next_proposal_id(&self) -> u32 {
        Self::id_to_u32(self.header.next_id)
    }

    fn proposal_count(&self) -> u32 {
        self.header.proposal_count()
    }

    fn list_proposals(&self, offset: Option<u32>, count: Option<u32>) -> Vec<u32> {
        list(
            self.header.proposal_ids().map(Self::id_to_u32),
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
        self.header.effective_ttl(&operation, requested_ttl)
    }

    fn get_operation_ttl(&self, kind: OperationKind) -> Nanoseconds {
        self.header.ttls.get(kind)
    }

    #[payable]
    fn create_proposal(
        &mut self,
        id: u32,
        operation: Operation,
        requested_ttl: Nanoseconds,
    ) -> Proposal<Operation> {
        near_sdk::assert_one_yocto();
        Self::assert_authorized(&operation);

        let effective_ttl = self.header.effective_ttl(&operation, requested_ttl);
        if effective_ttl > MAX_PROPOSAL_TTL {
            env::panic_str("Proposal TTL exceeds maximum allowed");
        }

        let proposal = self
            .header
            .create(
                u64::from(id),
                operation,
                Nanoseconds::near_timestamp(),
                env::predecessor_account_id(),
                effective_ttl,
            )
            .unwrap_or_reject();
        self.proposals.insert(id, proposal.clone());
        Event::Created {
            id,
            proposal: proposal.clone(),
        }
        .emit();
        proposal
    }

    #[payable]
    fn cancel_proposal(&mut self, id: u32) {
        near_sdk::assert_one_yocto();
        let operation = self.proposals.get(&id).unwrap_or_reject().operation.clone();
        Self::assert_authorized(&operation);

        self.header.cancel(u64::from(id)).unwrap_or_reject();
        let proposal = self.proposals.remove(&id).unwrap_or_reject();
        Event::Cancelled { id, proposal }.emit();
    }

    #[payable]
    #[allow(clippy::too_many_lines)]
    fn execute_proposal(&mut self, id: u32) {
        near_sdk::assert_one_yocto();

        let proposal = self.proposals.get(&id).unwrap_or_reject().clone();
        Self::assert_authorized(&proposal.operation);
        if let Operation::SetRole {
            account_id,
            role,
            set,
        } = &proposal.operation
        {
            Self::assert_can_set_role(account_id, *role, *set);
        }

        // Commit the authoritative transition first (validates, enforces
        // maturity, drops the id from the pending set), then fire effects.
        self.header
            .execute(u64::from(id), &proposal, Nanoseconds::near_timestamp())
            .unwrap_or_reject();
        let proposal = self.proposals.remove(&id).unwrap_or_reject();
        let operation = proposal.operation.clone();
        Event::Executed { id, proposal }.emit();

        let proxy_oracle_id = self.proxy_oracle_id.clone();

        match operation {
            Operation::SetProxy { id, proxy } => {
                ext_proxy_oracle_admin::ext(proxy_oracle_id)
                    .admin_set_proxy(id, proxy)
                    .detach();
            }
            Operation::ConfigureCircuitBreakers { id, config } => {
                ext_proxy_oracle_admin::ext(proxy_oracle_id)
                    .admin_configure_circuit_breakers(id, config)
                    .detach();
            }
            Operation::AddCircuitBreaker {
                id,
                breaker_id,
                breaker,
            } => {
                ext_proxy_oracle_admin::ext(proxy_oracle_id)
                    .admin_add_circuit_breaker(id, breaker_id, breaker)
                    .detach();
            }
            Operation::RemoveCircuitBreaker { id, breaker_id } => {
                ext_proxy_oracle_admin::ext(proxy_oracle_id)
                    .admin_remove_circuit_breaker(id, breaker_id)
                    .detach();
            }
            Operation::SetManualTrip {
                id,
                is_manually_tripped,
                metadata,
            } => {
                ext_proxy_oracle_admin::ext(proxy_oracle_id)
                    .admin_set_manual_trip(
                        id,
                        is_manually_tripped,
                        metadata.map(near_sdk::json_types::Base64VecU8),
                    )
                    .detach();
            }
            Operation::Rearm {
                id,
                breaker_id,
                armed_after_ns,
                accepted_history_source,
            } => {
                ext_proxy_oracle_admin::ext(proxy_oracle_id)
                    .admin_rearm(id, breaker_id, armed_after_ns, accepted_history_source)
                    .detach();
            }
            Operation::SetEnforced {
                id,
                breaker_id,
                is_enforced,
            } => {
                ext_proxy_oracle_admin::ext(proxy_oracle_id)
                    .admin_set_enforced(id, breaker_id, is_enforced)
                    .detach();
            }
            Operation::SetActionTtl { kind, new_ttl } => {
                self.header.ttls.set(kind, new_ttl);
            }
            Operation::SetRole {
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
            Operation::AdminUpgrade { code, migrate_args } => {
                ext_proxy_oracle_admin::ext(proxy_oracle_id)
                    .with_static_gas(Self::GAS_FOR_ADMIN_UPGRADE)
                    .admin_upgrade(code, migrate_args)
                    .detach();
            }
            Operation::AdminFunctionCall {
                method_name,
                args,
                attached_deposit,
                gas,
            } => {
                Promise::new(proxy_oracle_id)
                    .function_call(
                        method_name,
                        args.0,
                        NearToken::from_yoctonear(attached_deposit.0),
                        gas,
                    )
                    .detach();
            }
        }
    }
}

#[near]
#[allow(clippy::needless_pass_by_value)]
impl Contract {
    #[init]
    pub fn new(proxy_oracle_id: AccountId, admin_id: AccountId, ttls: TtlConfig) -> Self {
        let mut self_ = Self {
            header: Governance::new(ttls, MAX_PENDING_PROPOSALS),
            proposals: LookupMap::new(StorageKey::Proposals),
            proxy_oracle_id,
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
