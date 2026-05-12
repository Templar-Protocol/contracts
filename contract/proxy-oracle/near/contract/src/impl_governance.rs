use near_sdk::{assert_one_yocto, env, near, require};
use near_sdk_contract_tools::owner::Owner;
use templar_common::{contract::list, governance::Proposal, Nanoseconds, UnwrapReject};
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{
    CircuitBreakerSet, CircuitBreakerStatus,
};
use templar_proxy_oracle_near_common::governance::{
    CircuitBreakerUpdate, Operation, ProxyGovernanceInterface, MAX_CIRCUIT_BREAKERS_PER_PROXY,
};

use crate::{Contract, ContractExt};

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
                if let Some(proxy) = proxy {
                    self.proxies.insert(&id, &proxy);
                    if self.circuit_breakers.get(&id).is_none() {
                        self.circuit_breakers
                            .insert(&id, &CircuitBreakerSet::empty());
                    }
                } else {
                    self.proxies.remove(&id);
                    self.circuit_breakers.remove(&id);
                }
            }
            Operation::SetActionTtl { new_ttl } => {
                self.governance.ttl = new_ttl;
            }
            Operation::ConfigureCircuitBreakers { id, config } => {
                require!(self.proxies.get(&id).is_some(), "Proxy not found");
                let mut set = self
                    .circuit_breakers
                    .get(&id)
                    .unwrap_or_else(CircuitBreakerSet::empty);
                set.set_config(config);
                self.circuit_breakers.insert(&id, &set);
            }
            Operation::SetCircuitBreakerManualTrip {
                id,
                is_manually_tripped,
            } => {
                require!(self.proxies.get(&id).is_some(), "Proxy not found");
                let mut set = self
                    .circuit_breakers
                    .get(&id)
                    .unwrap_or_else(CircuitBreakerSet::empty);
                set.set_manual_trip(is_manually_tripped);
                self.circuit_breakers.insert(&id, &set);
            }
            Operation::AddCircuitBreaker {
                id,
                breaker_id,
                breaker,
            } => {
                require!(self.proxies.get(&id).is_some(), "Proxy not found");
                let mut set = self
                    .circuit_breakers
                    .get(&id)
                    .unwrap_or_else(CircuitBreakerSet::empty);
                require!(
                    set.breakers.len() < MAX_CIRCUIT_BREAKERS_PER_PROXY,
                    "Too many circuit breakers"
                );
                set.add(breaker_id, breaker).unwrap_or_reject();
                self.circuit_breakers.insert(&id, &set);
            }
            Operation::RemoveCircuitBreaker { id, breaker_id } => {
                let mut set = self
                    .circuit_breakers
                    .get(&id)
                    .unwrap_or_else(|| env::panic_str("Circuit breaker set not found"));
                set.remove(breaker_id).unwrap_or_reject();
                self.circuit_breakers.insert(&id, &set);
            }
            Operation::UpdateCircuitBreaker {
                id,
                breaker_id,
                update,
            } => {
                let mut set = self
                    .circuit_breakers
                    .get(&id)
                    .unwrap_or_else(|| env::panic_str("Circuit breaker set not found"));
                let breaker = set.get_mut(breaker_id).unwrap_or_reject();
                match update {
                    CircuitBreakerUpdate::SetEnforced { is_enforced } => {
                        breaker.is_enforced = is_enforced;
                    }
                    CircuitBreakerUpdate::Arm => {
                        breaker.status = CircuitBreakerStatus::Armed;
                    }
                    CircuitBreakerUpdate::Mute { until_ns } => {
                        breaker.status = CircuitBreakerStatus::Muted { until_ns };
                    }
                }
                self.circuit_breakers.insert(&id, &set);
            }
        }
    }
}
