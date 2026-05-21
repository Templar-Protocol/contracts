use near_sdk::{assert_one_yocto, env, near, require};
use near_sdk_contract_tools::{owner::Owner, rbac::Rbac};
use templar_common::{contract::list, governance::Proposal, Nanoseconds, UnwrapReject};
use templar_proxy_oracle_near_common::{
    convert::account_id_to_kernel,
    event::Event,
    governance::{Operation, ProxyGovernanceInterface, MAX_CIRCUIT_BREAKERS_PER_PROXY},
};

use crate::{emit_outcome, Contract, ContractExt};

#[near]
impl ProxyGovernanceInterface for Contract {
    fn gov_next_id(&self) -> u32 {
        self.governance.next_id
    }

    fn gov_ttl_ns(&self) -> Nanoseconds {
        self.governance.ttl
    }

    fn gov_count(&self) -> u32 {
        self.governance.proposals.len()
    }

    fn gov_list(&self, offset: Option<u32>, count: Option<u32>) -> Vec<u32> {
        list(self.governance.proposals.keys().copied(), offset, count)
    }

    fn gov_get(&self, id: u32) -> Option<Proposal<Operation>> {
        self.governance.proposals.get(&id).cloned()
    }

    #[payable]
    fn gov_create(&mut self, id: u32, operation: Operation) -> Proposal<Operation> {
        assert_one_yocto();
        self.assert_owner();

        self.governance
            .create(
                id,
                operation,
                Nanoseconds::near_timestamp(),
                env::predecessor_account_id(),
            )
            .unwrap_or_reject()
    }

    #[payable]
    fn gov_cancel(&mut self, id: u32) {
        assert_one_yocto();
        self.assert_owner();

        self.governance.cancel(id).unwrap_or_reject();
    }

    #[payable]
    fn gov_execute(&mut self, id: u32) {
        assert_one_yocto();
        self.assert_owner();

        match self
            .governance
            .execute(id, Nanoseconds::near_timestamp())
            .unwrap_or_reject()
        {
            Operation::SetProxy { id, proxy } => {
                self.state.set_proxy(id, proxy);
            }
            Operation::SetActionTtl { new_ttl } => {
                self.governance.ttl = new_ttl;
            }
            Operation::SetCircuitBreakerRole {
                account_id,
                role,
                is_granted,
            } => {
                if is_granted {
                    <Self as Rbac>::add_role(self, &account_id, &role);
                } else {
                    <Self as Rbac>::remove_role(self, &account_id, &role);
                }
                Event::CircuitBreakerRoleSet {
                    account_id,
                    role,
                    is_granted,
                }
                .emit();
            }
            Operation::ConfigureCircuitBreakers { id, config } => {
                let result = self
                    .state
                    .proxy_entry_mut(id)
                    .unwrap_or_else(|| env::panic_str("Proxy not found"))
                    .edit_circuit_breaker_set(|set| set.set_config(config));
                emit_outcome(id, result);
            }
            Operation::SetCircuitBreakerManualTrip {
                id,
                is_manually_tripped,
            } => {
                let result = self
                    .state
                    .proxy_entry_mut(id)
                    .unwrap_or_else(|| env::panic_str("Proxy not found"))
                    .edit_circuit_breaker_set(|set| {
                        set.set_manual_trip(
                            is_manually_tripped,
                            account_id_to_kernel(env::predecessor_account_id().as_ref()),
                            None,
                        )
                    });
                emit_outcome(id, result);
            }
            Operation::AddCircuitBreaker {
                id,
                breaker_id,
                breaker,
            } => {
                let result = self
                    .state
                    .proxy_entry_mut(id)
                    .unwrap_or_else(|| env::panic_str("Proxy not found"))
                    .edit_circuit_breaker_set(|set| {
                        require!(
                            set.breaker_count() < MAX_CIRCUIT_BREAKERS_PER_PROXY,
                            "Too many circuit breakers"
                        );
                        set.add(breaker_id, breaker)
                    })
                    .unwrap_or_reject();
                emit_outcome(id, result);
            }
            Operation::RemoveCircuitBreaker { id, breaker_id } => {
                let result = self
                    .state
                    .proxy_entry_mut(id)
                    .unwrap_or_else(|| env::panic_str("Proxy not found"))
                    .edit_circuit_breaker_set(|set| set.remove(breaker_id))
                    .unwrap_or_reject();
                emit_outcome(id, result);
            }
            Operation::UpdateCircuitBreaker {
                id,
                breaker_id,
                update,
            } => {
                let result = self
                    .state
                    .proxy_entry_mut(id)
                    .unwrap_or_else(|| env::panic_str("Proxy not found"))
                    .edit_circuit_breaker_set(|set| set.update(breaker_id, update))
                    .unwrap_or_reject();
                emit_outcome(id, result);
            }
        }
    }
}
