use feed_data::{FeedData, SerializableU256};
use near_sdk::{
    ext_contract,
    json_types::{Base64VecU8, U128, U64},
    near,
};

pub mod adapter;
pub mod config;
pub mod event;
pub mod feed_data;
mod utils;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json])]
pub struct GetPrices {
    pub timestamp: U64,
    pub prices: Vec<U128>,
}

#[ext_contract(ext_redstone)]
pub trait RedStoneContractInterface {
    fn unique_signer_threshold(&self) -> U64;
    fn get_prices(&self, feed_ids: Vec<String>, payload: Base64VecU8) -> GetPrices;
    fn read_prices(&self, feed_ids: Vec<String>) -> Vec<SerializableU256>;
    fn read_timestamp(&self, feed_id: String) -> U64;
    fn read_price_data_for_feed(&self, feed_id: String) -> &FeedData;
    fn read_price_data(&self, feed_ids: Vec<String>) -> Vec<&FeedData>;
    fn write_prices(&mut self, feed_ids: Vec<String>, payload: Base64VecU8);
}
