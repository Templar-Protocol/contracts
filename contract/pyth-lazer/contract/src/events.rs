use near_sdk::near;
use templar_common::oracle::lazer::FeedData;

/// NEP-297 events emitted by the adapter.
#[near(event_json(standard = "pyth-lazer-adapter"))]
pub enum PythLazerEvent {
    /// Emitted after a successful `update_price_feeds`, listing the feeds that were written
    /// (by Lazer feed id) together with their new data.
    #[event_version("1.0.0")]
    UpdatePrices { updated_feeds: Vec<(u32, FeedData)> },
}
