use blockchain_gateway_core::{common, storage};

use crate::client::{macros::contract_views, NearReadClient};

use super::ContractClient;

#[derive(Clone)]
pub struct StorageClient<'a> {
    pub(crate) inner: &'a NearReadClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl ContractClient for StorageClient<'_> {
    fn client(&self) -> &NearReadClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

impl StorageClient<'_> {
    contract_views! {
        pub fn storage_balance_bounds(storage::GetBalanceBoundsArgs) -> common::StorageBalanceBounds;
        pub fn storage_balance_of(storage::GetBalanceOfArgs) -> common::StorageBalance;
    }
}
