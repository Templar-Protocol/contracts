use near_sdk::{assert_one_yocto, env, near};
use near_sdk_contract_tools::owner::Owner;
use templar_common::{contract::list, governance::Proposal, UnwrapReject};
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_near_common::governance::{Operation, ProxyGovernanceInterface};

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
                } else {
                    self.proxies.remove(&id);
                }
            }
            Operation::SetActionTtl { new_ttl } => {
                self.governance.ttl = new_ttl;
            }
        }
    }
}
