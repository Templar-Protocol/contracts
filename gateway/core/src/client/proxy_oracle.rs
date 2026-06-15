use moka::sync::Cache;
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_types::ProxyOracle;
use templar_proxy_oracle_kernel::proxy::Proxy;
use templar_proxy_oracle_near_common::{input::Source, state::legacy::v0};

use crate::client::{
    cache::{config_cache, load_cached},
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

const PROXY_DEFINITION_CACHE_CAPACITY: u64 = 4_096;

#[derive(Clone)]
pub(crate) struct ProxyOracleClientCaches {
    pub definition: Cache<ProxyDefinitionCacheKey, std::sync::Arc<Option<Proxy<Source>>>>,
}

impl ProxyOracleClientCaches {
    pub fn new() -> Self {
        Self {
            definition: config_cache(PROXY_DEFINITION_CACHE_CAPACITY),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct ProxyDefinitionCacheKey {
    pub oracle_id: AccountId,
    pub price_identifier: PriceIdentifier,
}

#[derive(Clone)]
pub struct ProxyOracleClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for ProxyOracleClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }
    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

#[derive(serde::Serialize)]
pub struct ListProxiesArgs {
    pub offset: Option<u32>,
    pub count: Option<u32>,
}
#[derive(serde::Serialize)]
pub struct GetProxyArgs {
    pub id: PriceIdentifier,
}
#[derive(serde::Serialize)]
pub struct PriceFeedExistsArgs {
    pub price_identifier: PriceIdentifier,
}
#[derive(serde::Serialize)]
pub struct OwnerProposeArgs {
    pub account_id: Option<near_account_id::AccountId>,
}

impl ProxyOracleClient<'_> {
    pub async fn cached_get_proxy(
        &self,
        args: GetProxyArgs,
    ) -> crate::GatewayResult<Option<Proxy<Source>>> {
        load_cached(
            &self.inner.cache().proxy_oracle.definition,
            ProxyDefinitionCacheKey {
                oracle_id: self.contract_id.clone(),
                price_identifier: args.id,
            },
            {
                let near = self.inner.clone();
                let contract_id = self.contract_id.clone();
                move || async move { near.proxy_oracle(contract_id).get_proxy(args).await }
            },
        )
        .await
    }

    /// Fetch a proxy definition, normalizing the legacy (`< 0.2.0`) `v0::Proxy`
    /// shape into the unified `Proxy<Source>` so callers never see the
    /// pre-kernel representation. The oracle version is read from the (cached)
    /// NEP-330 `contract_source_metadata`.
    pub async fn get_proxy(
        &self,
        args: GetProxyArgs,
    ) -> crate::GatewayResult<Option<Proxy<Source>>> {
        let version = self
            .inner
            .contract(self.contract_id.clone())
            .version::<ProxyOracle>()
            .await?;
        let raw_args = serde_json::to_vec(&args)?;

        if version.proxy_is_kernelized() {
            crate::ReadNear::view_function(
                self.inner,
                self.contract_id.clone(),
                "get_proxy",
                raw_args,
            )
            .await
        } else {
            let legacy: Option<v0::Proxy> = crate::ReadNear::view_function(
                self.inner,
                self.contract_id.clone(),
                "get_proxy",
                raw_args,
            )
            .await?;
            Ok(legacy.map(Proxy::from))
        }
    }

    contract_views! {
        pub fn list_proxies(ListProxiesArgs) -> Vec<PriceIdentifier>;
        pub fn price_feed_exists(PriceFeedExistsArgs) -> bool;
        pub fn own_get_owner(()) -> Option<near_account_id::AccountId>;
        pub fn own_get_proposed_owner(()) -> Option<near_account_id::AccountId>;
    }

    contract_writes! {
        pub fn own_propose_owner(OwnerProposeArgs);
        pub fn own_accept_owner(());
        pub fn own_renounce_owner(());
    }
}
