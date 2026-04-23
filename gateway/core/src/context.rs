use std::{path::Path, sync::Arc};

use near_account_id::AccountId;
use near_api::NetworkConfig;
use templar_gateway_types::{ManagedAccountId, MarketId, RegistryId, UniversalAccountId};
use url::Url;

use crate::{
    client::{
        account::AccountClient, chain::ChainClient, contract::ContractClient, ft::FtClient,
        lst_oracle::LstOracleClient, market::MarketClient, mt::MtClient,
        proxy_oracle::ProxyOracleClient, pyth_oracle::PythOracleClient,
        redstone_oracle::RedStoneOracleClient, ref_finance::RefFinanceClient,
        registry::RegistryClient, storage::StorageClient, token::TokenClient, tx::TxClient,
        universal_account::UniversalAccountClient,
    },
    GatewayResult, NearClient, PythHttpClient, RedStoneBridgeClient,
};
use templar_common::asset::{AssetClass, FungibleAsset};

#[derive(Debug, Clone)]
pub struct GatewayContext {
    near: NearClient,
    pyth_http: PythHttpClient,
    redstone_bridge: RedStoneBridgeClient,
}

impl GatewayContext {
    pub fn new(
        network: NetworkConfig,
        pyth_hermes_url: Url,
        node_path: &Path,
    ) -> GatewayResult<Self> {
        let near = NearClient::new(network);
        let pyth_http = PythHttpClient::new(pyth_hermes_url);
        let redstone_bridge = RedStoneBridgeClient::new(node_path)?;

        Ok(Self {
            near,
            pyth_http,
            redstone_bridge,
        })
    }

    pub fn near(&self) -> &NearClient {
        &self.near
    }

    pub fn network(&self) -> &NetworkConfig {
        self.near.network()
    }

    pub fn chain(&self) -> ChainClient<'_> {
        self.near.chain()
    }

    pub fn account(&self) -> AccountClient<'_> {
        self.near.account()
    }

    pub fn contract(&self, contract_id: AccountId) -> ContractClient<'_> {
        self.near.contract(contract_id)
    }

    pub fn ft(&self, contract_id: AccountId) -> FtClient<'_> {
        self.near.ft(contract_id)
    }

    pub fn mt(&self, contract_id: AccountId) -> MtClient<'_> {
        self.near.mt(contract_id)
    }

    pub fn token<T: AssetClass>(&self, asset: FungibleAsset<T>) -> TokenClient<'_> {
        self.near.token(asset)
    }

    pub fn proxy_oracle(&self, contract_id: AccountId) -> ProxyOracleClient<'_> {
        self.near.proxy_oracle(contract_id)
    }

    pub fn pyth_oracle(&self, contract_id: AccountId) -> PythOracleClient<'_> {
        self.near.pyth_oracle(contract_id)
    }

    pub fn redstone_oracle(&self, contract_id: AccountId) -> RedStoneOracleClient<'_> {
        self.near.redstone_oracle(contract_id)
    }

    pub fn ref_finance(&self, contract_id: AccountId) -> RefFinanceClient<'_> {
        self.near.ref_finance(contract_id)
    }

    pub fn lst_oracle(&self, contract_id: AccountId) -> LstOracleClient<'_> {
        self.near.lst_oracle(contract_id)
    }

    pub fn registry(&self, contract_id: RegistryId) -> RegistryClient<'_> {
        self.near.registry(contract_id)
    }

    pub fn market(&self, contract_id: MarketId) -> MarketClient<'_> {
        self.near.market(contract_id)
    }

    pub fn storage(&self, contract_id: AccountId) -> StorageClient<'_> {
        self.near.storage(contract_id)
    }

    pub fn universal_account(&self, contract_id: UniversalAccountId) -> UniversalAccountClient<'_> {
        self.near.universal_account(contract_id)
    }

    pub fn tx(
        &self,
        signer_account_id: ManagedAccountId,
        signer: Arc<near_api::Signer>,
    ) -> TxClient<'_> {
        self.near.tx(signer_account_id, signer)
    }

    pub fn pyth_http(&self) -> &PythHttpClient {
        &self.pyth_http
    }

    pub fn redstone_bridge(&self) -> &RedStoneBridgeClient {
        &self.redstone_bridge
    }
}
