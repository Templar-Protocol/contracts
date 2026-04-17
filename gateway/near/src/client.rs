pub mod account;
pub mod chain;
pub mod contract;
pub mod ft;
pub mod macros;
pub mod market;
pub mod proxy_oracle;
pub mod registry;
pub mod storage;
pub mod tx;
pub mod universal_account;

use account::AccountClient;
use blockchain_gateway_core::{
    ManagedAccountId, MarketId, NearGas, NearToken, RegistryId, UniversalAccountId,
};
use chain::ChainClient;
use contract::ContractClient;
use ft::FtClient;
use market::MarketClient;
use near_account_id::{AccountId, AccountIdRef};
use near_api::NetworkConfig;
use proxy_oracle::ProxyOracleClient;
use registry::RegistryClient;
use std::sync::Arc;
use storage::StorageClient;
use tx::TxClient;
use universal_account::UniversalAccountClient;

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

    pub fn ft(&self, contract_id: AccountId) -> FtClient<'_> {
        FtClient {
            inner: self,
            contract_id,
        }
    }

    pub fn proxy_oracle(&self, contract_id: AccountId) -> ProxyOracleClient<'_> {
        ProxyOracleClient {
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
}
