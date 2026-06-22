use near_sdk::near;

use crate::FeedData;

/// NEP-297 events emitted by the adapter.
#[near(event_json(standard = "pyth-pro-adapter"))]
pub enum PythProEvent {
    /// Emitted after a successful `update_price_feeds`, listing the feeds that were written
    /// (by Lazer feed id) together with their new data.
    #[event_version("1.0.0")]
    UpdatePrices { updated_feeds: Vec<(u32, FeedData)> },
}
