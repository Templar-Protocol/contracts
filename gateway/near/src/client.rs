mod chain;
mod macros;
mod market;
mod registry;
mod storage;
mod tx;
mod universal_account;

use blockchain_gateway_core::{
    rpc::common::ContractArgs, ManagedAccountId, MarketId, RegistryId, UniversalAccountId,
};
use near_account_id::{AccountId, AccountIdRef};
use near_api::{types::Data, Contract, NetworkConfig};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

use crate::error::{GatewayError, GatewayResult};

pub use chain::ChainClient;
pub use market::MarketClient;
pub use registry::RegistryClient;
pub use storage::StorageClient;
pub use tx::TxClient;
pub use universal_account::UniversalAccountClient;

trait ContractClient {
    fn client(&self) -> &NearClient;
    fn contract_id(&self) -> &AccountIdRef;
}

#[derive(Debug, Clone)]
pub struct NearClient {
    network: NetworkConfig,
}

#[derive(Clone)]
pub struct ManagedSigner {
    pub signer: Arc<near_api::Signer>,
    pub key_count: usize,
}

impl ManagedSigner {
    pub async fn new(secret_keys: impl IntoIterator<Item = near_api::SecretKey>) -> Option<Self> {
        let mut secret_keys = secret_keys.into_iter();
        let signer = near_api::Signer::from_secret_key(secret_keys.next()?).ok()?;
        let mut key_count = 1;
        for secret_key in secret_keys {
            signer.add_secret_key_to_pool(secret_key).await.ok()?;
            key_count += 1;
        }
        Some(Self { signer, key_count })
    }
}

impl NearClient {
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

    pub fn storage(&self, contract_id: AccountId) -> StorageClient<'_> {
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

    pub fn tx(
        &self,
        signer_account_id: ManagedAccountId,
        signer: Arc<near_api::Signer>,
    ) -> TxClient<'_> {
        TxClient {
            inner: self,
            signer_account_id,
            signer,
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

fn contract_args_bytes(args: blockchain_gateway_core::common::ContractArgs) -> Vec<u8> {
    match args {
        blockchain_gateway_core::common::ContractArgs::Json(value) => {
            serde_json::to_vec(&value).expect("contract args should serialize")
        }
        blockchain_gateway_core::common::ContractArgs::Raw(bytes) => bytes.0,
    }
}

trait IntoNearTxExecutionStatus {
    fn into_near(self) -> near_api::types::TxExecutionStatus;
}

impl IntoNearTxExecutionStatus for blockchain_gateway_core::common::TxExecutionStatus {
    fn into_near(self) -> near_api::types::TxExecutionStatus {
        match self {
            blockchain_gateway_core::common::TxExecutionStatus::None => {
                near_api::types::TxExecutionStatus::None
            }
            blockchain_gateway_core::common::TxExecutionStatus::Included => {
                near_api::types::TxExecutionStatus::Included
            }
            blockchain_gateway_core::common::TxExecutionStatus::ExecutedOptimistic => {
                near_api::types::TxExecutionStatus::ExecutedOptimistic
            }
            blockchain_gateway_core::common::TxExecutionStatus::IncludedFinal => {
                near_api::types::TxExecutionStatus::IncludedFinal
            }
            blockchain_gateway_core::common::TxExecutionStatus::Executed => {
                near_api::types::TxExecutionStatus::Executed
            }
            blockchain_gateway_core::common::TxExecutionStatus::Final => {
                near_api::types::TxExecutionStatus::Final
            }
        }
    }
}
