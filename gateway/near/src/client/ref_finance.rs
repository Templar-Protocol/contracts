use blockchain_gateway_core::U128;
use near_account_id::AccountId;

use crate::client::{macros::contract_views, NearClient};

use super::BoundContractClient;

#[derive(Clone)]
pub struct RefFinanceClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: AccountId,
}

impl BoundContractClient for RefFinanceClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PoolInfo {
    pub token_account_ids: Vec<AccountId>,
    pub shares_total_supply: U128,
}

#[derive(serde::Serialize)]
pub struct GetPoolsArgs {
    pub from_index: Option<u64>,
    pub limit: Option<u64>,
}

impl RefFinanceClient<'_> {
    contract_views! {
        pub fn get_pools(GetPoolsArgs) -> Vec<PoolInfo>;
    }
}
