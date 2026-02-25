use near_sdk::{near, AccountId};

use super::feed_data::FeedData;

#[near(event_json(standard = "redstone-adapter"))]
pub enum RedStoneEvent {
    #[event_version("1.0.0")]
    WritePrices {
        updater: AccountId,
        updated_feeds: Vec<FeedData>,
    },
}
