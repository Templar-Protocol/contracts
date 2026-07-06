use moka::sync::Cache;
use near_account_id::AccountId;
use templar_common::oracle::pyth::{FeedIdOracleResponse, PriceIdentifier};

use crate::client::{
    cache::{config_cache, load_cached},
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

const FEED_MAPPING_CACHE_CAPACITY: u64 = 4_096;

#[derive(Clone)]
pub(crate) struct PythProOracleClientCaches {
    pub feed_mapping: Cache<FeedMappingCacheKey, std::sync::Arc<Option<u32>>>,
}

impl PythProOracleClientCaches {
    pub fn new() -> Self {
        Self {
            feed_mapping: config_cache(FEED_MAPPING_CACHE_CAPACITY),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct FeedMappingCacheKey {
    pub oracle_id: AccountId,
    pub price_identifier: PriceIdentifier,
}

#[derive(Clone)]
pub struct PythProOracleClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for PythProOracleClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }
    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

/// Arguments for the Pyth Pro adapter's permissionless `update_price_feeds`
/// write method. The field name `payload` matches the adapter's parameter name
/// (`contract/pyth-pro/contract/src/lib.rs: update_price_feeds(payload:
/// Base64VecU8)`); renaming it would silently break the on-chain deserializer.
#[derive(serde::Serialize)]
pub struct UpdatePriceFeedsArgs {
    pub payload: near_sdk::json_types::Base64VecU8,
}

/// Feed-id-keyed EMA read against the adapter (`list_ema_prices_by_feed_id_no_older_than`). Lazer
/// feeds are addressed by their native `u32` id, so this takes feed ids rather than
/// `PriceIdentifier`s.
#[derive(serde::Serialize)]
pub struct ListEmaPricesByFeedIdNoOlderThanArgs {
    pub feed_ids: Vec<u32>,
    pub age: u64,
}

/// Arguments for the adapter's `get_feed_mapping` view (`price_identifier` matches the
/// adapter's parameter name, `contract/pyth-pro/contract/src/feed_map.rs`).
#[derive(serde::Serialize)]
pub struct GetFeedMappingArgs {
    pub price_identifier: PriceIdentifier,
}

impl PythProOracleClient<'_> {
    /// Cached resolution of a consumer `PriceIdentifier` to its Lazer `u32` feed id
    /// (`None` when the adapter has no mapping for it).
    pub async fn cached_get_feed_mapping(
        &self,
        args: GetFeedMappingArgs,
    ) -> crate::GatewayResult<Option<u32>> {
        load_cached(
            &self.inner.cache().pyth_pro_oracle.feed_mapping,
            FeedMappingCacheKey {
                oracle_id: self.contract_id.clone(),
                price_identifier: args.price_identifier,
            },
            {
                let near = self.inner.clone();
                let contract_id = self.contract_id.clone();
                move || async move {
                    near.pyth_pro_oracle(contract_id)
                        .get_feed_mapping(args)
                        .await
                }
            },
        )
        .await
    }

    contract_views! {
        pub fn list_ema_prices_by_feed_id_no_older_than(ListEmaPricesByFeedIdNoOlderThanArgs) -> FeedIdOracleResponse;
        pub fn get_feed_mapping(GetFeedMappingArgs) -> Option<u32>;
    }

    contract_writes! {
        pub fn update_price_feeds(UpdatePriceFeedsArgs);
    }
}
