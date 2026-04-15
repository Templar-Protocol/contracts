use blockchain_gateway_core::storage;
use near_contract_standards::storage_management::{StorageBalance, StorageBalanceBounds};

use crate::client::{macros::contract_views, NearClient};

use super::ContractClient;

#[derive(Clone)]
pub struct StorageClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl ContractClient for StorageClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

impl StorageClient<'_> {
    contract_views! {
        pub fn storage_balance_bounds(storage::GetBalanceBoundsArgs) -> StorageBalanceBounds;
        pub fn storage_balance_of(storage::GetBalanceOfArgs) -> Option<StorageBalance>;
    }
}
