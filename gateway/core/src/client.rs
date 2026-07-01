pub mod account;
pub mod cache;
pub mod chain;
pub mod contract;
pub mod ft;
pub mod lst_oracle;
pub mod macros;
pub mod market;
pub mod mt;
pub mod proxy_governance;
pub mod proxy_oracle;
pub mod pyth_oracle;
pub mod redstone_oracle;
pub mod ref_finance;
pub mod registry;
pub mod storage;
pub mod token;
pub mod tx;
pub mod universal_account;
pub mod vault;

use std::sync::Arc;

use account::AccountClient;
use cache::NearClientCache;
use chain::ChainClient;
use contract::ContractClient;
use ft::FtClient;
use lst_oracle::LstOracleClient;
use market::MarketClient;
use mt::MtClient;
use near_account_id::{AccountId, AccountIdRef};
use near_api::NetworkConfig;
use proxy_governance::ProxyGovernanceClient;
use proxy_oracle::ProxyOracleClient;
use pyth_oracle::PythOracleClient;
use redstone_oracle::RedStoneOracleClient;
use ref_finance::RefFinanceClient;
use registry::RegistryClient;
use storage::StorageClient;
use templar_common::asset::{AssetClass, FungibleAsset};
use templar_gateway_types::{ManagedAccountId, NearGas, NearToken};
use token::TokenClient;
use tx::TxClient;
use universal_account::UniversalAccountClient;
use vault::VaultClient;

trait BoundContractClient {
    fn client(&self) -> &NearClient;
    fn contract_id(&self) -> &AccountIdRef;
}

#[derive(Clone)]
pub struct ContractWriteOptions {
    signer_account_id: ManagedAccountId,
    wait_until: templar_gateway_types::common::TxExecutionStatus,
    gas: NearGas,
    deposit: NearToken,
}

impl ContractWriteOptions {
    pub fn new(signer_account_id: ManagedAccountId) -> Self {
        Self {
            signer_account_id,
            wait_until: templar_gateway_types::common::TxExecutionStatus::ExecutedOptimistic,
            gas: NearGas::from_tgas(30),
            deposit: NearToken::from_yoctonear(0),
        }
    }

    #[must_use]
    pub fn wait_until(
        mut self,
        wait_until: templar_gateway_types::common::TxExecutionStatus,
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

#[derive(Clone)]
pub struct NearClient {
    network: NetworkConfig,
    cache: Arc<NearClientCache>,
}

impl std::fmt::Debug for NearClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NearClient")
            .field("network", &self.network)
            .finish_non_exhaustive()
    }
}

impl NearClient {
    pub fn new(network: NetworkConfig) -> Self {
        Self {
            network,
            cache: Arc::new(NearClientCache::new()),
        }
    }

    pub fn network(&self) -> &NetworkConfig {
        &self.network
    }

    pub(crate) fn cache(&self) -> &NearClientCache {
        &self.cache
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

    pub fn mt(&self, contract_id: AccountId) -> MtClient<'_> {
        MtClient {
            inner: self,
            contract_id,
        }
    }

    pub fn token<T: AssetClass>(&self, asset: FungibleAsset<T>) -> TokenClient<'_> {
        TokenClient::new(self, asset)
    }

    pub fn proxy_oracle(&self, contract_id: AccountId) -> ProxyOracleClient<'_> {
        ProxyOracleClient {
            inner: self,
            contract_id,
        }
    }

    pub fn proxy_governance(&self, contract_id: AccountId) -> ProxyGovernanceClient<'_> {
        ProxyGovernanceClient {
            inner: self,
            contract_id,
        }
    }

    pub fn pyth_oracle(&self, contract_id: AccountId) -> PythOracleClient<'_> {
        PythOracleClient {
            inner: self,
            contract_id,
        }
    }

    pub fn redstone_oracle(&self, contract_id: AccountId) -> RedStoneOracleClient<'_> {
        RedStoneOracleClient {
            inner: self,
            contract_id,
        }
    }

    pub fn ref_finance(&self, contract_id: AccountId) -> RefFinanceClient<'_> {
        RefFinanceClient {
            inner: self,
            contract_id,
        }
    }

    pub fn lst_oracle(&self, contract_id: AccountId) -> LstOracleClient<'_> {
        LstOracleClient {
            inner: self,
            contract_id,
        }
    }

    pub fn registry(&self, contract_id: AccountId) -> RegistryClient<'_> {
        RegistryClient {
            inner: self,
            contract_id,
        }
    }

    pub fn market(&self, contract_id: AccountId) -> MarketClient<'_> {
        MarketClient {
            inner: self,
            contract_id,
        }
    }

    pub fn vault(&self, contract_id: AccountId) -> VaultClient<'_> {
        VaultClient {
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

    pub fn universal_account(&self, contract_id: AccountId) -> UniversalAccountClient<'_> {
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
