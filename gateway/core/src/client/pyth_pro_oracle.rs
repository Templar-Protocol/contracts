use templar_common::oracle::pyth::FeedIdOracleResponse;

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

impl PythProOracleClient<'_> {
    contract_views! {
        pub fn list_ema_prices_by_feed_id_no_older_than(ListEmaPricesByFeedIdNoOlderThanArgs) -> FeedIdOracleResponse;
    }

    contract_writes! {
        pub fn update_price_feeds(UpdatePriceFeedsArgs);
    }
}
