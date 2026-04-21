use std::io::ErrorKind;

use blockchain_gateway_core::Version;
use moka::sync::Cache;
use near_account_id::AccountId;
use near_api::Contract;
use near_contract_standards::contract_metadata::ContractSourceMetadata;
use serde::de::DeserializeOwned;

use crate::{
    client::{
        cache::{immutable_cache, load_cached},
        macros::contract_views,
        NearClient,
    },
    GatewayError, GatewayResult,
};

use super::BoundContractClient;

const CONTRACT_METADATA_CACHE_CAPACITY: u64 = 256;

#[derive(Clone)]
pub(crate) struct ContractClientCaches {
    pub contract_source_metadata: Cache<AccountId, std::sync::Arc<ContractSourceMetadata>>,
}

impl ContractClientCaches {
    pub fn new() -> Self {
        Self {
            contract_source_metadata: immutable_cache(CONTRACT_METADATA_CACHE_CAPACITY),
        }
    }
}

#[derive(Clone)]
pub struct ContractClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for ContractClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

impl ContractClient<'_> {
    pub async fn view_function<T>(&self, method_name: &str, args: Vec<u8>) -> GatewayResult<T>
    where
        T: DeserializeOwned + Send + Sync + 'static,
    {
        Ok(Contract(self.contract_id.clone())
            .call_function_raw(method_name, args)
            .read_only()
            .fetch_from(self.inner.network())
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?
            .data)
    }

    pub async fn cached_contract_source_metadata(&self) -> GatewayResult<ContractSourceMetadata> {
        load_cached(
            &self.inner.cache().contract.contract_source_metadata,
            self.contract_id.clone(),
            {
                let near = self.inner.clone();
                let contract_id = self.contract_id.clone();
                move || async move {
                    near.contract(contract_id)
                        .contract_source_metadata(())
                        .await
                }
            },
        )
        .await
    }

    pub async fn version<T>(&self) -> GatewayResult<Version<T>> {
        let meta = self.cached_contract_source_metadata().await?;
        let Some(ver_str) = meta.version else {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                format!("contract {} missing version", self.contract_id),
            )
            .into());
        };
        Ok(ver_str
            .parse()
            .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?)
    }

    contract_views! {
        pub fn contract_source_metadata(()) -> ContractSourceMetadata;
    }
}
