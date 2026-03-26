use std::collections::HashMap;

use near_sdk::{
    ext_contract,
    json_types::{Base64VecU8, U64},
    near,
};

/// All RedStone feeds report 8 decimals.
pub const DECIMALS: i32 = 8;

mod adapter;
pub use adapter::*;
pub mod config;
pub use config::Config;
mod event;
pub use event::*;
mod feed_data;
pub use feed_data::*;
mod feed_id;
pub use feed_id::*;
mod serializable_u256;
pub use serializable_u256::*;

use crate::time::Nanoseconds;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json])]
pub struct GetPrices {
    pub timestamp: U64,
    pub prices: HashMap<FeedId, SerializableU256>,
}

#[ext_contract(ext_redstone)]
pub trait RedStoneContractInterface {
    fn unique_signer_threshold(&self) -> U64;
    fn get_prices(&self, feed_ids: Vec<FeedId>, payload: Base64VecU8) -> GetPrices;
    fn read_prices(&self, feed_ids: Vec<FeedId>) -> HashMap<FeedId, SerializableU256>;
    fn read_timestamp(&self, feed_id: FeedId) -> Option<Nanoseconds>;
    fn read_price_data_for_feed(&self, feed_id: FeedId) -> Option<FeedData>;
    fn read_price_data(&self, feed_ids: Vec<FeedId>) -> HashMap<FeedId, FeedData>;
    fn write_prices(&mut self, feed_ids: Vec<FeedId>, payload: Base64VecU8);
}
