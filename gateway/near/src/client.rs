pub mod account;
pub mod chain;
pub mod contract;
pub mod macros;
pub mod market;
pub mod registry;
pub mod storage;
pub mod tx;
pub mod universal_account;

use account::AccountClient;
use blockchain_gateway_core::{
    rpc::common::ContractArgs, ManagedAccountId, MarketId, NearGas, NearToken, RegistryId,
    UniversalAccountId,
};
use chain::ChainClient;
use contract::ContractClient;
use market::MarketClient;
use near_account_id::{AccountId, AccountIdRef};
use near_api::{types::Data, Contract, NetworkConfig};
use registry::RegistryClient;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use storage::StorageClient;
use tx::TxClient;
use universal_account::UniversalAccountClient;

use crate::error::{GatewayError, GatewayResult};

trait BoundContractClient {
    fn client(&self) -> &NearClient;
    fn contract_id(&self) -> &AccountIdRef;
}

#[derive(Clone)]
pub struct ContractWriteOptions {
    signer_account_id: ManagedAccountId,
    signer: Arc<near_api::Signer>,
    wait_until: blockchain_gateway_core::common::TxExecutionStatus,
    gas: NearGas,
    deposit: NearToken,
}

impl ContractWriteOptions {
    pub fn new(signer_account_id: ManagedAccountId, signer: Arc<near_api::Signer>) -> Self {
        Self {
            signer_account_id,
            signer,
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::default(),
            gas: NearGas::from_tgas(30),
            deposit: NearToken::from_yoctonear(0),
        }
    }

    #[must_use]
    pub fn wait_until(
        mut self,
        wait_until: blockchain_gateway_core::common::TxExecutionStatus,
    ) -> Self {
        self.wait_until = wait_until;
        self
    }

    #[must_use]
    pub fn gas(mut self, gas: NearGas) -> Self {
        self.gas = gas;
        self
    }

    #[must_use]
    pub fn tgas(mut self, tgas: u64) -> Self {
        self.gas = NearGas::from_tgas(tgas);
        self
    }

    #[must_use]
    pub fn deposit(mut self, deposit: NearToken) -> Self {
        self.deposit = deposit;
        self
    }

    #[must_use]
    pub fn one_yocto(mut self) -> Self {
        self.deposit = NearToken::from_yoctonear(1);
        self
    }
}

#[derive(Debug, Clone)]
pub struct NearClient {
    network: NetworkConfig,
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

    pub fn account(&self) -> AccountClient<'_> {
        AccountClient { inner: self }
    }

    pub fn contract(&self, contract_id: AccountId) -> ContractClient<'_> {
        ContractClient {
            inner: self,
            contract_id,
        }
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
