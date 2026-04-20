use std::collections::HashMap;

use near_sdk::json_types::Base64VecU8;
use templar_common::oracle::redstone::{FeedData, FeedId};

use crate::client::{
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

#[derive(Clone)]
pub struct RedStoneOracleClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for RedStoneOracleClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }
    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

#[derive(serde::Serialize)]
pub struct ReadPriceDataArgs {
    pub feed_ids: Vec<FeedId>,
}

#[derive(serde::Serialize)]
pub struct WritePricesArgs {
    pub feed_ids: Vec<FeedId>,
    pub payload: Base64VecU8,
}

impl RedStoneOracleClient<'_> {
    contract_views! {
        pub fn read_price_data(ReadPriceDataArgs) -> HashMap<FeedId, FeedData>;
    }

    contract_writes! {
        pub fn write_prices(WritePricesArgs);
    }
}
