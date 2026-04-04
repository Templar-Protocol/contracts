mod chain;
mod macros;
mod market;
mod registry;
mod storage;
mod universal_account;

use blockchain_gateway_core::{
    rpc::common::ContractArgs, MarketId, RegistryId, UniversalAccountId,
};
use near_account_id::{AccountId, AccountIdRef};
use near_api::{types::Data, Contract, NetworkConfig};
use serde::Serialize;
use serde_json::Value;

use crate::error::{GatewayError, GatewayResult};

pub use chain::ChainClient;
pub use market::MarketClient;
pub use registry::RegistryClient;
pub use storage::StorageClient;
pub use universal_account::UniversalAccountClient;

trait ContractClient {
    fn client(&self) -> &NearReadClient;
    fn contract_id(&self) -> &AccountIdRef;
}

#[derive(Debug, Clone)]
pub struct NearReadClient {
    network: NetworkConfig,
}

impl NearReadClient {
    pub fn new(network: NetworkConfig) -> Self {
        Self { network }
    }

    pub fn network(&self) -> &NetworkConfig {
        &self.network
    }

    pub fn chain(&self) -> ChainClient<'_> {
        ChainClient { inner: self }
    }

    pub fn registry(&self, contract_id: RegistryId) -> RegistryClient<'_> {
        RegistryClient {
            inner: self,
            contract_id,
        }
    }

    pub fn market(&self, contract_id: MarketId) -> MarketClient<'_> {
        MarketClient {
            inner: self,
            contract_id,
        }
    }

    pub fn storage(&self, contract_id: near_account_id::AccountId) -> StorageClient<'_> {
        StorageClient {
            inner: self,
            contract_id,
        }
    }

    pub fn universal_account(&self, contract_id: UniversalAccountId) -> UniversalAccountClient<'_> {
        UniversalAccountClient {
            inner: self,
            contract_id,
        }
    }

    async fn view_json<T>(
        &self,
        contract_id: AccountId,
        method_name: &str,
        args: impl Serialize,
    ) -> GatewayResult<Data<T>>
    where
        T: serde::de::DeserializeOwned + Send + Sync + 'static,
    {
        Contract(contract_id)
            .call_function(method_name, args)
            .read_only()
            .fetch_from(&self.network)
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))
    }

    async fn view_raw<T>(
        &self,
        contract_id: AccountId,
        method_name: &str,
        args: Vec<u8>,
    ) -> GatewayResult<Data<T>>
    where
        T: serde::de::DeserializeOwned + Send + Sync + 'static,
    {
        Contract(contract_id)
            .call_function_raw(method_name, args)
            .read_only()
            .fetch_from(&self.network)
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))
    }

    async fn view_value(
        &self,
        contract_id: AccountId,
        method_name: &str,
        args: &ContractArgs,
    ) -> GatewayResult<Data<Value>> {
        match args {
            ContractArgs::Json(value) => {
                self.view_json(contract_id, method_name, value.clone())
                    .await
            }
            ContractArgs::Raw(bytes) => {
                self.view_raw(contract_id, method_name, bytes.0.clone())
                    .await
            }
        }
    }
}
