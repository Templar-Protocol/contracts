use moka::sync::Cache;
use near_account_id::AccountId as OwnedAccountId;
use near_contract_standards::storage_management::{
    StorageBalance, StorageBalanceBounds as NearStorageBalanceBounds,
};
use near_sdk::AccountId;

use crate::client::{
    cache::{config_cache, is_method_not_found},
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

const STORAGE_BOUNDS_CACHE_CAPACITY: u64 = 512;

#[derive(Clone)]
pub(crate) struct StorageClientCaches {
    pub balance_bounds_if_supported:
        Cache<OwnedAccountId, std::sync::Arc<Option<StorageBalanceBoundsView>>>,
}

impl StorageClientCaches {
    pub fn new() -> Self {
        Self {
            balance_bounds_if_supported: config_cache(STORAGE_BOUNDS_CACHE_CAPACITY),
        }
    }
}

#[derive(serde::Serialize)]
pub struct StorageBalanceOfArgs {
    pub account_id: AccountId,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StorageBalanceBoundsView {
    pub min: near_sdk::NearToken,
    pub max: Option<near_sdk::NearToken>,
}

impl From<NearStorageBalanceBounds> for StorageBalanceBoundsView {
    fn from(bounds: NearStorageBalanceBounds) -> Self {
        Self {
            min: bounds.min,
            max: bounds.max,
        }
    }
}

impl From<StorageBalanceBoundsView> for NearStorageBalanceBounds {
    fn from(bounds: StorageBalanceBoundsView) -> Self {
        Self {
            min: bounds.min,
            max: bounds.max,
        }
    }
}

impl From<&NearStorageBalanceBounds> for StorageBalanceBoundsView {
    fn from(bounds: &NearStorageBalanceBounds) -> Self {
        Self {
            min: bounds.min,
            max: bounds.max,
        }
    }
}

impl From<&StorageBalanceBoundsView> for NearStorageBalanceBounds {
    fn from(bounds: &StorageBalanceBoundsView) -> Self {
        Self {
            min: bounds.min,
            max: bounds.max,
        }
    }
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
    pub async fn cached_storage_balance_bounds_if_supported(
        &self,
    ) -> crate::GatewayResult<Option<StorageBalanceBoundsView>> {
        let cache = &self.inner.cache().storage.balance_bounds_if_supported;
        if let Some(bounds) = cache.get(&self.contract_id) {
            return Ok((*bounds).clone());
        }

        let bounds = match self.storage_balance_bounds(()).await {
            Ok(bounds) => Some(bounds),
            Err(error) if is_method_not_found(&error) => None,
            Err(error) => return Err(error),
        };

        let cached = bounds.clone();
        cache.insert(self.contract_id.clone(), std::sync::Arc::new(bounds));
        Ok(cached)
    }

    pub async fn cached_storage_balance_bounds(
        &self,
    ) -> crate::GatewayResult<StorageBalanceBoundsView> {
        self.cached_storage_balance_bounds_if_supported()
            .await?
            .ok_or_else(|| {
                crate::GatewayError::NearQuery("MethodNotFound: storage_balance_bounds".to_owned())
            })
    }

    contract_views! {
        pub fn storage_balance_bounds(()) -> StorageBalanceBoundsView;
        pub fn storage_balance_of(StorageBalanceOfArgs) -> Option<StorageBalance>;
    }

    contract_writes! {
        pub(crate) fn storage_deposit(StorageDepositArgs);
        pub(crate) fn storage_unregister(StorageUnregisterArgs);
    }
}
