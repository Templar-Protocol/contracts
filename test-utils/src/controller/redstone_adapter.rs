use std::collections::HashMap;

use near_sdk::json_types::{Base64VecU8, U64};
use templar_common::oracle::redstone::{
    config::Config, FeedData, FeedId, GetPrices, SerializableU256,
};

use crate::define;

use super::ContractController;

pub trait RedStoneAdapterController: ContractController {
    define! {
        #[view] fn get_config() -> Config;
        #[view] fn unique_signer_threshold() -> U64;
        #[view] fn get_prices(feed_ids: Vec<FeedId>, payload: Base64VecU8) -> GetPrices;
        #[view] fn read_prices(feed_ids: Vec<FeedId>) -> HashMap<FeedId, SerializableU256>;
        #[view] fn read_timestamp(feed_id: FeedId) -> U64;
        #[view] fn read_price_data_for_feed(feed_id: FeedId) -> FeedData;
        #[view] fn read_price_data(feed_ids: Vec<FeedId>) -> HashMap<FeedId, FeedData>;

        #[call(exec)]
        fn write_prices(feed_ids: Vec<FeedId>, payload: Base64VecU8);
    }
}
