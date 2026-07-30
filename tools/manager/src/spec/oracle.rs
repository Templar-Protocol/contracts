//! One side of a market's asset pair: the token, and the oracle sources that
//! price it.
//!
//! Keeping those together is the point. Today the asset lives in
//! `market-args.json` and its feeds in `proxy-*.json`, and nothing checks that
//! the feed prices the asset it is paired with.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::{
    asset::{AssetClass, FungibleAsset},
    Nanoseconds,
};
use templar_proxy_oracle_kernel::proxy::{
    aggregator::{
        method::median::{MedianHigh, MedianLow},
        Aggregator,
    },
    freshness_filter::FreshnessFilter,
    Proxy, WeightedSource,
};
use templar_proxy_oracle_near_common::{input::Source, request::OracleRequest};

use super::serde_util::{duration_opt, fungible_asset};

/// The asset and the feeds that price it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssetSpec<A: AssetClass> {
    /// `nep141:<contract>` or `nep245:<contract>:<token>`.
    #[serde(with = "fungible_asset", bound = "A: AssetClass")]
    #[schemars(with = "String")]
    pub asset: FungibleAsset<A>,

    /// Ticker. Never sent on chain — it exists so a preflight can check that the
    /// sources below actually price this asset (ENG-543).
    pub symbol: String,

    /// Overrides the token's on-chain metadata. Required when that metadata is
    /// absent or malformed, which has happened for at least one bridged asset
    /// whose deployer never populated `ft_metadata`. Consumed by ENG-541.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,

    pub aggregator: AggregatorSpec,

    /// Minimum sources that must resolve for a price to be produced.
    pub min_sources: u32,

    pub sources: Vec<SourceSpec>,

    /// Defaults to the market's `price_maximum_age`, so the market-side and
    /// proxy-side staleness bounds cannot silently diverge.
    #[serde(
        default,
        with = "duration_opt",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<String>")]
    pub max_age: Option<Nanoseconds>,

    /// How far into the future a price timestamp may sit before it is rejected.
    /// Defaults to [`DEFAULT_MAX_CLOCK_DRIFT`].
    #[serde(
        default,
        with = "duration_opt",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<String>")]
    pub max_clock_drift: Option<Nanoseconds>,
}

/// Tolerance for a price timestamped in the future, matching every deployed
/// alpha proxy.
///
/// Deliberately *not* derived from `price_maximum_age`: staleness and clock
/// drift bound opposite directions, and a market willing to accept a 60s-old
/// price has said nothing about accepting one dated 60s from now.
pub const DEFAULT_MAX_CLOCK_DRIFT: Nanoseconds = Nanoseconds::from_ns(10_000_000_000);

/// How multiple sources collapse into one price.
///
/// `Priority` is deliberately absent: its on-chain form holds *unweighted*
/// sources and has no `min_sources`, so a spec naming it would silently discard
/// both fields — precisely the class of mistake this tool exists to catch. No
/// alpha market uses it. Adding it means giving it validation that rejects the
/// fields it cannot honour, not just another variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AggregatorSpec {
    MedianLow,
    MedianHigh,
}

/// A weighted price source, flattened for authoring.
///
/// The on-chain shape is `Source::Request(OracleRequest::Lazer(LazerRequest))`
/// — three levels of externally-tagged enum, which TOML renders as
/// `[collateral.sources.source.Request.Lazer]`. This is a deliberate parallel
/// model; [`super::tests`] round-trips it against the on-chain encoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceSpec {
    Lazer {
        oracle: near_account_id::AccountId,
        feed_id: u32,
        weight: u32,
    },
    RedStone {
        oracle: near_account_id::AccountId,
        price_id: String,
        weight: u32,
    },
}

impl SourceSpec {
    /// The account serving this source, for the reachability check in ENG-541.
    pub fn oracle_id(&self) -> &near_account_id::AccountId {
        match self {
            Self::Lazer { oracle, .. } | Self::RedStone { oracle, .. } => oracle,
        }
    }

    pub const fn weight(&self) -> u32 {
        match self {
            Self::Lazer { weight, .. } | Self::RedStone { weight, .. } => *weight,
        }
    }
}

impl From<SourceSpec> for WeightedSource<Source> {
    fn from(spec: SourceSpec) -> Self {
        let (request, weight) = match spec {
            SourceSpec::Lazer {
                oracle,
                feed_id,
                weight,
            } => (OracleRequest::lazer(oracle, feed_id), weight),
            SourceSpec::RedStone {
                oracle,
                price_id,
                weight,
            } => (OracleRequest::redstone(oracle, price_id), weight),
        };

        Self {
            source: Source::Request(request),
            weight,
        }
    }
}

impl<A: AssetClass> AssetSpec<A> {
    /// Build the on-chain proxy configuration.
    ///
    /// An unset `max_age` falls back to `default_max_age` (the market's
    /// `price_maximum_age`); an unset `max_clock_drift` falls back to
    /// [`DEFAULT_MAX_CLOCK_DRIFT`], which is deliberately independent of it.
    pub fn into_proxy(self, default_max_age: Nanoseconds) -> Proxy<Source> {
        let sources = self.sources.into_iter().map(WeightedSource::from);
        // `Median::new` defaults `min_sources` to 1; the field is public and the
        // spec always states it, so set it rather than accept the default.
        let aggregator = match self.aggregator {
            AggregatorSpec::MedianLow => {
                let mut median = MedianLow::new(sources);
                median.min_sources = self.min_sources;
                Aggregator::MedianLow(median)
            }
            AggregatorSpec::MedianHigh => {
                let mut median = MedianHigh::new(sources);
                median.min_sources = self.min_sources;
                Aggregator::MedianHigh(median)
            }
        };

        Proxy::new(
            aggregator,
            FreshnessFilter::new(
                Some(self.max_age.unwrap_or(default_max_age)),
                Some(self.max_clock_drift.unwrap_or(DEFAULT_MAX_CLOCK_DRIFT)),
            ),
        )
    }
}
