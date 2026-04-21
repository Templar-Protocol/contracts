use near_contract_standards::storage_management::{StorageBalance, StorageBalanceBounds};
use near_sdk::AccountId;

use crate::client::{
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

#[derive(serde::Serialize)]
pub struct StorageBalanceOfArgs {
    pub account_id: AccountId,
}

#[derive(serde::Serialize)]
pub(crate) struct StorageDepositArgs {
    pub account_id: Option<near_account_id::AccountId>,
    pub registration_only: bool,
}

#[derive(serde::Serialize)]
pub(crate) struct StorageUnregisterArgs {
    pub force: bool,
}

#[derive(Clone)]
pub struct StorageClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for StorageClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

impl StorageClient<'_> {
    contract_views! {
        pub fn storage_balance_bounds(()) -> StorageBalanceBounds;
        pub fn storage_balance_of(StorageBalanceOfArgs) -> Option<StorageBalance>;
    }

    contract_writes! {
        pub(crate) fn storage_deposit(StorageDepositArgs);
        pub(crate) fn storage_unregister(StorageUnregisterArgs);
    }
}
