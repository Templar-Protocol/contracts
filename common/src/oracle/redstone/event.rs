use near_sdk::AccountId;
use near_sdk_contract_tools::Nep297;

use super::feed_data::FeedData;

#[derive(Clone, Debug, Nep297)]
#[nep297(standard = "redstone-adapter", version = "1.0.0")]
#[near_sdk::near(serializers = [json])]
pub enum RedStoneEvent {
    WritePrices {
        updater: AccountId,
        updated_feeds: Vec<FeedData>,
    },
}
