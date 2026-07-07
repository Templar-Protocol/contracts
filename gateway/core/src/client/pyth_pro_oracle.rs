use templar_common::oracle::pyth::{FeedIdOracleResponse, PriceIdentifier};

use crate::client::{
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

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
/// adapter's parameter name, `contract/pyth-pro/contract/src/feed_map.rs`). Used only to
/// probe/identify the adapter during contract-kind detection — the adapter is otherwise
/// addressed by native feed id, not `PriceIdentifier`.
#[derive(serde::Serialize)]
pub struct GetFeedMappingArgs {
    pub price_identifier: PriceIdentifier,
}

impl PythProOracleClient<'_> {
    contract_views! {
        pub fn list_ema_prices_by_feed_id_no_older_than(ListEmaPricesByFeedIdNoOlderThanArgs) -> FeedIdOracleResponse;
        pub fn get_feed_mapping(GetFeedMappingArgs) -> Option<u32>;
    }

    contract_writes! {
        pub fn update_price_feeds(UpdatePriceFeedsArgs);
    }
}
