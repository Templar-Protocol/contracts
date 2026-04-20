use std::collections::HashMap;

use templar_common::oracle::pyth::{Price, PriceIdentifier};

use crate::client::{
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

#[derive(Clone)]
pub struct PythOracleClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for PythOracleClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }
    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

#[derive(serde::Serialize)]
pub struct PriceFeedExistsArgs {
    pub price_identifier: PriceIdentifier,
}

#[derive(serde::Serialize)]
pub struct ListEmaPricesNoOlderThanArgs {
    pub price_ids: Vec<PriceIdentifier>,
    pub age: u64,
}

#[derive(serde::Serialize)]
pub struct UpdatePriceFeedsArgs {
    pub data: String,
}

impl PythOracleClient<'_> {
    contract_views! {
        pub fn price_feed_exists(PriceFeedExistsArgs) -> bool;
        pub fn list_ema_prices_no_older_than(ListEmaPricesNoOlderThanArgs) -> HashMap<PriceIdentifier, Option<Price>>;
    }

    contract_writes! {
        pub fn update_price_feeds(UpdatePriceFeedsArgs);
    }
}
