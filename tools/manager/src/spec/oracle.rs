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
use templar_proxy_oracle_near_common::{
    input::{ProxyPriceTransformer, Source},
    price_transformer::Call,
    request::OracleRequest,
};

use super::serde_util::{duration_opt, fungible_asset};

/// The asset and the feeds that price it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssetSpec<A: AssetClass> {
    /// `nep141:<contract>` or `nep245:<contract>:<token>`.
    #[serde(with = "fungible_asset", bound = "A: AssetClass")]
    #[schemars(with = "String")]
    pub asset: FungibleAsset<A>,

    /// Ticker. Never sent on chain, so `market export` cannot recover it and
    /// leaves it unset. It exists so the reference cross-check can confirm the
    /// sources below actually price this asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,

    /// How to find this asset on the reference price API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<ReferenceAsset>,

    /// The identifier this asset's price is served under, when `oracle` is
    /// `direct`. Unused for a proxy, which serves the constants this tool owns.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "price_id")]
    #[schemars(with = "Option<String>")]
    pub price_id: Option<templar_common::oracle::pyth::PriceIdentifier>,

    /// Overrides `market.reference_tolerance` for this asset.
    ///
    /// Not a nicety: one flat band false-positives on an LST trading at a
    /// premium to its underlying, or on a thinly-traded bridged token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub reference_tolerance: Option<templar_common::Decimal>,

    /// Overrides the token's on-chain metadata. Required when that metadata is
    /// absent or malformed, which has happened for at least one bridged asset
    /// whose deployer never populated `ft_metadata`. Consumed by ENG-541.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,

    /// `None` for a direct market, which aggregates nothing. Optional rather
    /// than defaulted: a proxy spec that omits it would otherwise deploy
    /// `MedianLow` silently, where every alpha market reads its borrow side at
    /// `median_high` — a permissive difference nobody authored. Absence has to
    /// be *representable* for `config.oracle_mode` to reject it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregator: Option<AggregatorSpec>,

    /// Minimum sources that must resolve for a price to be produced.
    #[serde(default)]
    pub min_sources: u32,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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

/// How to find an asset on the reference price API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ReferenceAsset {
    /// Resolve `symbol` against the API. Ambiguity is an error listing the
    /// candidates, never a first match.
    ByTicker,
    /// A pinned id, skipping resolution.
    ///
    /// This is how a wrapped asset records what it is priced as: `FXRP` pinned
    /// to `ripple` asserts that the bridged token tracks XRP — an assumption
    /// that otherwise lives only in someone's head.
    CoinGecko { id: String },
    /// No third-party listing exists. The reason is recorded so an absent check
    /// is visible in review rather than silently not running.
    Unlisted { reason: String },
}

impl Default for ReferenceAsset {
    fn default() -> Self {
        Self::ByTicker
    }
}

/// How multiple sources collapse into one price.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AggregatorSpec {
    #[default]
    MedianLow,
    MedianHigh,
    /// Takes the first source that answers, in order. Carries no weights and no
    /// `min_sources`; both are refused on a `priority` asset.
    Priority,
}

impl AggregatorSpec {
    /// Whether this aggregator weighs its sources and honors a minimum count.
    pub const fn is_weighted(self) -> bool {
        matches!(self, Self::MedianLow | Self::MedianHigh)
    }
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
        /// Required by the median aggregators, refused by `priority`, which
        /// ranks its sources by position instead.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weight: Option<u32>,
    },
    RedStone {
        oracle: near_account_id::AccountId,
        price_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weight: Option<u32>,
    },
    Pyth {
        oracle: near_account_id::AccountId,
        #[serde(with = "price_id_hex")]
        #[schemars(with = "String")]
        price_id: templar_common::oracle::pyth::PriceIdentifier,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weight: Option<u32>,
    },
    /// A liquid-staking token, priced as its underlying scaled by the exchange
    /// rate a view on the staking contract returns.
    Lst {
        /// Pyth oracle serving the *underlying* asset.
        oracle: near_account_id::AccountId,
        #[serde(with = "price_id_hex")]
        #[schemars(with = "String")]
        price_id: templar_common::oracle::pyth::PriceIdentifier,
        /// Staking contract, and the no-argument view returning the exchange
        /// rate in `decimals` native units.
        contract: near_account_id::AccountId,
        method: String,
        decimals: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weight: Option<u32>,
    },
}

/// Gas for the exchange-rate view, matching both deployed LST sources.
///
/// Constant rather than a spec field: it is a property of a no-argument view,
/// not a per-market decision. Export refuses a transformer that differs rather
/// than emitting a spec that would redeploy with the wrong value.
pub const LST_CALL_GAS: near_sdk::Gas = near_sdk::Gas::from_tgas(3);

impl SourceSpec {
    /// The account serving this source, for the reachability check in ENG-541.
    pub fn oracle_id(&self) -> &near_account_id::AccountId {
        match self {
            Self::Lazer { oracle, .. }
            | Self::RedStone { oracle, .. }
            | Self::Pyth { oracle, .. }
            | Self::Lst { oracle, .. } => oracle,
        }
    }

    /// How this source reads in a report.
    pub fn describe(&self) -> String {
        match self {
            Self::Lazer { feed_id, .. } => format!("lazer feed {feed_id}"),
            Self::RedStone { price_id, .. } => format!("redstone `{price_id}`"),
            Self::Pyth { price_id, .. } => format!("pyth `{}`", hex::encode(price_id.0)),
            Self::Lst {
                contract, method, ..
            } => format!("lst `{contract}.{method}`"),
        }
    }

    /// The authored weight, if any. `priority` sources carry none.
    pub const fn weight(&self) -> Option<u32> {
        match self {
            Self::Lazer { weight, .. }
            | Self::RedStone { weight, .. }
            | Self::Pyth { weight, .. }
            | Self::Lst { weight, .. } => *weight,
        }
    }
}

impl From<SourceSpec> for WeightedSource<Source> {
    fn from(spec: SourceSpec) -> Self {
        // `priority` carries no weights; the field is validated away for that
        // aggregator, and the value here is discarded when the sources are
        // unwrapped into a plain list.
        let (source, weight) = match spec {
            SourceSpec::Lazer {
                oracle,
                feed_id,
                weight,
            } => (
                Source::Request(OracleRequest::lazer(oracle, feed_id)),
                weight,
            ),
            SourceSpec::RedStone {
                oracle,
                price_id,
                weight,
            } => (
                Source::Request(OracleRequest::redstone(oracle, price_id)),
                weight,
            ),
            SourceSpec::Pyth {
                oracle,
                price_id,
                weight,
            } => (Source::Request(pyth_request(oracle, price_id)), weight),
            SourceSpec::Lst {
                oracle,
                price_id,
                contract,
                method,
                decimals,
                weight,
            } => (
                Source::Transformer(ProxyPriceTransformer::lst(
                    pyth_request(oracle, price_id),
                    decimals,
                    Call::new(&contract, method, (), LST_CALL_GAS),
                )),
                weight,
            ),
        };

        Self {
            source,
            weight: weight.unwrap_or(1),
        }
    }
}

fn pyth_request(
    oracle_id: near_account_id::AccountId,
    price_id: templar_common::oracle::pyth::PriceIdentifier,
) -> OracleRequest {
    OracleRequest::Pyth(templar_proxy_oracle_near_common::request::PythRequest {
        oracle_id,
        price_id,
    })
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
        // `config.oracle_mode` refuses a proxy spec that states no aggregator,
        // so by here it is present; the fallback keeps this total rather than
        // panicking on a path the checks already closed.
        let aggregator = match self.aggregator.unwrap_or_default() {
            AggregatorSpec::Priority => Aggregator::Priority(
                templar_proxy_oracle_kernel::proxy::aggregator::method::priority::Priority::new(
                    sources.map(|weighted: WeightedSource<Source>| weighted.source),
                ),
            ),
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

/// A Pyth price identifier as hex, matching the form the chain stores.
mod price_id {
    use serde::{Deserialize as _, Deserializer, Serializer};
    use templar_common::oracle::pyth::PriceIdentifier;

    #[expect(clippy::ref_option, reason = "serde's serialize_with signature")]
    pub fn serialize<S: Serializer>(
        value: &Option<PriceIdentifier>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(id) => serializer.serialize_str(&hex::encode(id.0)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PriceIdentifier>, D::Error> {
        let Some(text) = Option::<String>::deserialize(deserializer)? else {
            return Ok(None);
        };
        let bytes = hex::decode(&text).map_err(serde::de::Error::custom)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("a price identifier is 32 bytes of hex"))?;
        Ok(Some(PriceIdentifier(bytes)))
    }
}

/// A required price identifier as hex. The `price_id` module beside it handles
/// the optional case on `AssetSpec`; the difference is only `Option`.
mod price_id_hex {
    use serde::{Deserialize as _, Deserializer, Serializer};
    use templar_common::oracle::pyth::PriceIdentifier;

    pub fn serialize<S: Serializer>(
        value: &PriceIdentifier,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(value.0))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PriceIdentifier, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = hex::decode(&text).map_err(serde::de::Error::custom)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("a price identifier is 32 bytes of hex"))?;
        Ok(PriceIdentifier(bytes))
    }
}
