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

/// JSON view of one verified feed returned by the adapter's `verify_update` — the full Lazer
/// property set (not just the Pyth-compatible subset). Price-like values are raw `i64` mantissas
/// (interpret with `exponent`); `_ns` timestamps are nanoseconds. `None` = property absent.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub struct ParsedFeedView {
    pub feed_id: u32,
    pub price: Option<I64>,
    pub best_bid_price: Option<I64>,
    pub best_ask_price: Option<I64>,
    pub publisher_count: Option<u16>,
    pub exponent: Option<i16>,
    pub confidence: Option<I64>,
    pub funding_rate: Option<I64>,
    pub funding_timestamp_ns: Option<Nanoseconds>,
    pub funding_rate_interval_ns: Option<Nanoseconds>,
    pub market_session: Option<i16>,
    pub ema_price: Option<I64>,
    pub ema_confidence: Option<I64>,
    pub feed_update_timestamp_ns: Option<Nanoseconds>,
}

/// JSON view of a verified update returned by the adapter's `verify_update`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub struct VerifiedUpdateView {
    /// The trusted ed25519 signer public key (hex).
    #[serde(
        serialize_with = "hex::serde::serialize",
        deserialize_with = "hex::serde::deserialize"
    )]
    pub signer: [u8; 32],
    pub channel_id: u8,
    pub timestamp_ns: Nanoseconds,
    pub feeds: Vec<ParsedFeedView>,
}

/// Fallibly build a storable [`FeedData`] from a verified feed, owning every intrinsic validity
/// rule. Returns `None` for missing price or exponent, missing or invalid spot/EMA confidence,
/// missing EMA price, or an effective publish timestamp more than `max_ahead_s` seconds beyond
/// `now`. Anti-replay is relational and handled by the caller.
///
/// EMA is required: a spot-only signed payload must never overwrite a stored feed and drop its
/// EMA. The adapter's stateless `verify_update` does not call this projection and permits
/// spot-only payloads.
pub fn feed_data_from_parsed(
    parsed: &ParsedFeedView,
    package: Nanoseconds,
    now: Nanoseconds,
    max_ahead_s: u64,
) -> Option<FeedData> {
    let price = parsed.price?.0;
    let exponent = parsed.exponent?;
    let conf = require_confidence(parsed.confidence.map(|value| value.0))?;

    // Effective per-feed publish time: FeedUpdateTimestamp when present, else the payload's.
    let publish_time_ns = parsed.feed_update_timestamp_ns.unwrap_or(package);

    // The verifier only bounds the package timestamp; reject a per-feed time too far ahead.
    if publish_time_ns.as_secs() > now.as_secs().saturating_add(max_ahead_s) {
        return None;
    }

    // Never synthesize EMA from spot: half-specified EMA skips the whole feed.
    let ema = EmaData {
        price: I64(parsed.ema_price?.0),
        conf: U64(require_confidence(
            parsed.ema_confidence.map(|value| value.0),
        )?),
    };

    Some(FeedData {
        price: I64(price),
        conf: U64(conf),
        ema,
        expo: i32::from(exponent),
        publish_time_ns,
    })
}

/// A confidence is usable only when explicitly present and strictly positive. On the Lazer wire
/// zero is indistinguishable from absent, so reject both rather than invent a precise-looking zero.
fn require_confidence(confidence: Option<i64>) -> Option<u64> {
    confidence
        .and_then(|value| u64::try_from(value).ok())
        .filter(|&value| value > 0)
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
    use near_sdk::serde_json::{self, json};
    use rstest::rstest;

    fn parsed_feed() -> ParsedFeedView {
        ParsedFeedView {
            feed_id: 7,
            price: Some(I64(123_456)),
            best_bid_price: Some(I64(123_450)),
            best_ask_price: Some(I64(123_460)),
            publisher_count: Some(4),
            exponent: Some(-8),
            confidence: Some(I64(50)),
            funding_rate: Some(I64(-2)),
            funding_timestamp_ns: Some(Nanoseconds::from_ns(98_123_456_789)),
            funding_rate_interval_ns: Some(Nanoseconds::from_ns(3_456_789)),
            market_session: Some(1),
            ema_price: Some(I64(123_000)),
            ema_confidence: Some(I64(40)),
            feed_update_timestamp_ns: Some(Nanoseconds::from_ns(99_987_654_321)),
        }
    }

    #[rstest]
    #[case::valid(Some(123_456), Some(-8), Some(123_000), true)]
    #[case::missing_spot(None, Some(-8), Some(123_000), false)]
    #[case::missing_exponent(Some(123_456), None, Some(123_000), false)]
    #[case::missing_ema(Some(123_456), Some(-8), None, false)]
    fn projection_requires_spot_exponent_and_ema(
        #[case] price: Option<i64>,
        #[case] exponent: Option<i16>,
        #[case] ema_price: Option<i64>,
        #[case] valid: bool,
    ) {
        let parsed = ParsedFeedView {
            price: price.map(I64),
            exponent,
            ema_price: ema_price.map(I64),
            ..parsed_feed()
        };
        let projected = feed_data_from_parsed(
            &parsed,
            Nanoseconds::from_secs(98),
            Nanoseconds::from_secs(100),
            2,
        );
        let expected = valid.then_some(FeedData {
            price: I64(123_456),
            conf: U64(50),
            ema: EmaData {
                price: I64(123_000),
                conf: U64(40),
            },
            expo: -8,
            publish_time_ns: Nanoseconds::from_ns(99_987_654_321),
        });
        assert_eq!(projected, expected);
    }

    #[rstest]
    #[case::valid(Some(50), Some(40), true)]
    #[case::missing_spot(None, Some(40), false)]
    #[case::zero_spot(Some(0), Some(40), false)]
    #[case::negative_spot(Some(-1), Some(40), false)]
    #[case::missing_ema(Some(50), None, false)]
    #[case::zero_ema(Some(50), Some(0), false)]
    #[case::negative_ema(Some(50), Some(-1), false)]
    fn projection_requires_positive_confidences(
        #[case] confidence: Option<i64>,
        #[case] ema_confidence: Option<i64>,
        #[case] valid: bool,
    ) {
        let parsed = ParsedFeedView {
            confidence: confidence.map(I64),
            ema_confidence: ema_confidence.map(I64),
            ..parsed_feed()
        };
        let projected = feed_data_from_parsed(
            &parsed,
            Nanoseconds::from_secs(98),
            Nanoseconds::from_secs(100),
            2,
        );
        let expected = valid.then_some(FeedData {
            price: I64(123_456),
            conf: U64(50),
            ema: EmaData {
                price: I64(123_000),
                conf: U64(40),
            },
            expo: -8,
            publish_time_ns: Nanoseconds::from_ns(99_987_654_321),
        });
        assert_eq!(projected, expected);
    }

    #[rstest]
    #[case::package_fallback(None, 98_123_456_789, 2, Some(98_123_456_789))]
    #[case::feed_overrides_package(Some(99_987_654_321), 103_000_000_000, 2, Some(99_987_654_321))]
    #[case::too_far_ahead(Some(103_000_000_000), 98_000_000_000, 2, None)]
    #[case::whole_second_boundary(Some(102_999_999_999), 98_000_000_000, 2, Some(102_999_999_999))]
    #[case::future_package(None, 103_000_000_000, 2, None)]
    #[case::saturating_ahead(Some(u64::MAX), 98_000_000_000, u64::MAX, Some(u64::MAX))]
    fn projection_preserves_effective_timestamp_and_tolerance(
        #[case] feed_ns: Option<u64>,
        #[case] package_ns: u64,
        #[case] max_ahead_s: u64,
        #[case] expected_ns: Option<u64>,
    ) {
        let parsed = ParsedFeedView {
            feed_update_timestamp_ns: feed_ns.map(Nanoseconds::from_ns),
            ..parsed_feed()
        };
        let projected = feed_data_from_parsed(
            &parsed,
            Nanoseconds::from_ns(package_ns),
            Nanoseconds::from_secs(100),
            max_ahead_s,
        );
        let expected = expected_ns.map(|publish_ns| FeedData {
            price: I64(123_456),
            conf: U64(50),
            ema: EmaData {
                price: I64(123_000),
                conf: U64(40),
            },
            expo: -8,
            publish_time_ns: Nanoseconds::from_ns(publish_ns),
        });
        assert_eq!(projected, expected);
    }

    #[test]
    fn verified_update_preserves_deployed_json_contract() {
        let view = VerifiedUpdateView {
            signer: [0xab; 32],
            channel_id: 3,
            timestamp_ns: Nanoseconds::from_ns(100_123_456_789),
            feeds: vec![parsed_feed()],
        };
        let expected = json!({
            "signer": "abababababababababababababababababababababababababababababababab",
            "channel_id": 3,
            "timestamp_ns": "100123456789",
            "feeds": [{
                "feed_id": 7,
                "price": "123456",
                "best_bid_price": "123450",
                "best_ask_price": "123460",
                "publisher_count": 4,
                "exponent": -8,
                "confidence": "50",
                "funding_rate": "-2",
                "funding_timestamp_ns": "98123456789",
                "funding_rate_interval_ns": "3456789",
                "market_session": 1,
                "ema_price": "123000",
                "ema_confidence": "40",
                "feed_update_timestamp_ns": "99987654321"
            }]
        });
        assert_eq!(serde_json::to_value(&view).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<VerifiedUpdateView>(expected).unwrap(),
            view,
        );
    }

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
