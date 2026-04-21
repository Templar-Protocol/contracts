use moka::sync::Cache;
use near_account_id::AccountId;
use templar_common::{
    governance::Proposal,
    oracle::{
        proxy::{governance::Operation, Proxy},
        pyth::PriceIdentifier,
    },
    time::Nanoseconds,
};

use crate::client::{
    cache::{config_cache, load_cached},
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

const PROXY_DEFINITION_CACHE_CAPACITY: u64 = 4_096;

#[derive(Clone)]
pub(crate) struct ProxyOracleClientCaches {
    pub definition: Cache<ProxyDefinitionCacheKey, std::sync::Arc<Option<Proxy>>>,
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
pub struct GovGetArgs {
    pub id: u32,
}
#[derive(serde::Serialize)]
pub struct GovCreateArgs {
    pub id: u32,
    pub operation: Operation,
}
#[derive(serde::Serialize)]
pub struct GovActionArgs {
    pub id: u32,
}
#[derive(serde::Serialize)]
pub struct GovListArgs {
    pub offset: Option<u32>,
    pub count: Option<u32>,
}
#[derive(serde::Serialize)]
pub struct OwnerProposeArgs {
    pub account_id: Option<near_account_id::AccountId>,
}

impl ProxyOracleClient<'_> {
    pub async fn cached_get_proxy(
        &self,
        args: GetProxyArgs,
    ) -> crate::GatewayResult<Option<Proxy>> {
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

    contract_views! {
        pub fn list_proxies(ListProxiesArgs) -> Vec<PriceIdentifier>;
        pub fn get_proxy(GetProxyArgs) -> Option<Proxy>;
        pub fn price_feed_exists(PriceFeedExistsArgs) -> bool;
        pub fn gov_next_id(()) -> u32;
        pub fn gov_ttl_ns(()) -> Nanoseconds;
        pub fn gov_count(()) -> u32;
        pub fn gov_list(GovListArgs) -> Vec<u32>;
        pub fn gov_get(GovGetArgs) -> Option<Proposal<Operation>>;
        pub fn own_get_owner(()) -> Option<near_account_id::AccountId>;
        pub fn own_get_proposed_owner(()) -> Option<near_account_id::AccountId>;
    }

    contract_writes! {
        pub(crate) fn gov_create(GovCreateArgs);
        pub(crate) fn gov_cancel(GovActionArgs);
        pub(crate) fn gov_execute(GovActionArgs);
        pub(crate) fn own_propose_owner(OwnerProposeArgs);
        pub(crate) fn own_accept_owner(());
        pub(crate) fn own_renounce_owner(());
    }
}
