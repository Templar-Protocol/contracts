//! Pyth Lazer adapter feed data — the native, feed-id-keyed shape the `contract/pyth-lazer`
//! adapter stores and serves. The adapter is a pure store-and-serve oracle: it hands back the raw
//! [`FeedData`], and each consumer (the proxy-oracle's `Lazer` source, the gateway) projects it to
//! a [`pyth::Price`] itself, mirroring [`redstone::FeedData::to_pyth_price`](super::redstone::FeedData).
//! Freshness is likewise the consumer's concern; the adapter applies no age filter on reads.

use std::collections::HashMap;

use near_sdk::{
    ext_contract,
    json_types::{I64, U64},
    near,
};
use templar_primitives::time::Nanoseconds;

use crate::oracle::pyth::{self, PythTimestamp};

/// Response shape of the adapter's feed-id-keyed read (`get_feeds_data`): the raw stored feed for
/// each requested `u32` feed id (`None` when the feed is absent).
pub type FeedDataResponse = HashMap<u32, Option<FeedData>>;

/// EMA price/confidence for a feed, following the same fixed-point `expo` as the spot price.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct EmaData {
    pub price: I64,
    pub conf: U64,
}

/// The latest stored data for one Lazer feed. Prices/exponent follow the Pyth fixed-point
/// convention (`value * 10^expo`); the timestamp is stored as [`Nanoseconds`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct FeedData {
    pub price: I64,
    pub conf: U64,
    /// EMA data. The adapter's stateful storage path requires it, so a stored feed always carries
    /// EMA; it is never synthesized from spot.
    pub ema: EmaData,
    pub expo: i32,
    /// Per-feed publish time in nanoseconds. The `_ns` suffix marks the unit for JSON consumers
    /// (the [`Nanoseconds`] type is erased in JSON).
    pub publish_time_ns: Nanoseconds,
}

impl FeedData {
    /// Build a Pyth [`Price`](pyth::Price) from a `(price, conf)` pair using this feed's exponent
    /// and publish time. `None` if the publish time cannot be represented as a [`PythTimestamp`].
    fn to_price(&self, price: I64, conf: U64) -> Option<pyth::Price> {
        Some(pyth::Price {
            price,
            conf,
            expo: self.expo,
            publish_time: PythTimestamp::try_from_time(self.publish_time_ns)?,
        })
    }

    /// Spot [`Price`](pyth::Price) projection.
    pub fn to_pyth_price(&self) -> Option<pyth::Price> {
        self.to_price(self.price, self.conf)
    }

    /// EMA [`Price`](pyth::Price) projection (the form the proxy-oracle's `Lazer` source consumes).
    pub fn to_ema_price(&self) -> Option<pyth::Price> {
        self.to_price(self.ema.price, self.ema.conf)
    }
}

/// Feed-id-keyed read ABI of the Pyth Lazer adapter (`contract/pyth-lazer`). Feeds are addressed
/// by their native `u32` id; the adapter serves the raw stored [`FeedData`] and the consumer
/// projects it (mirroring the RedStone adapter, which serves [`redstone::FeedData`](super::redstone::FeedData)).
#[ext_contract(ext_pyth_lazer)]
pub trait PythLazer {
    fn get_feeds_data(&self, feed_ids: Vec<u32>) -> FeedDataResponse;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(price: i64, ema: i64, conf: u64, publish_s: u64) -> FeedData {
        FeedData {
            price: I64(price),
            conf: U64(conf),
            ema: EmaData {
                price: I64(ema),
                conf: U64(conf),
            },
            expo: -8,
            publish_time_ns: Nanoseconds::from_secs(publish_s),
        }
    }

    #[test]
    fn spot_and_ema_projections_use_the_right_mantissa() {
        let feed = feed(123_456, 123_000, 50, 1_700_000_000);

        let spot = feed.to_pyth_price().unwrap();
        assert_eq!(spot.price.0, 123_456);
        assert_eq!(spot.conf.0, 50);
        assert_eq!(spot.expo, -8);
        assert_eq!(spot.publish_time.as_secs(), 1_700_000_000);

        let ema = feed.to_ema_price().unwrap();
        assert_eq!(ema.price.0, 123_000);
        assert_eq!(ema.conf.0, 50);
        assert_eq!(ema.expo, -8);
        assert_eq!(ema.publish_time.as_secs(), 1_700_000_000);
    }
}
