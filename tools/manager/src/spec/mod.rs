//! The declarative market deployment spec.
//!
//! One TOML file replaces the four the retired deploy read (`env.sh`,
//! `market-args.json`, `proxy-collateral.json`, `proxy-borrow.json`). The point
//! is not brevity — it is that every value which must agree is now written
//! exactly once. Anything derivable is derived here rather than declared:
//! account ids, price identifiers, and the proxy freshness bounds.
//!
//! Everything in this module is offline. Building and checking a spec must never
//! need a network, which is what lets `market export` (ENG-540) round-trip
//! specs in unit tests.

pub mod check;
pub mod export;
pub mod extends;
pub mod journal;
pub mod oracle;
pub mod plan;
mod serde_util;

use anyhow::Context as _;
use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::{
    asset::{BorrowAsset, CollateralAsset},
    fee::{Fee, TimeBasedFee},
    interest_rate_strategy::InterestRateStrategy,
    market::{
        AmountRange, MarketConfiguration, PriceOracleConfiguration, ValidAmountRange, YieldWeights,
    },
    oracle::pyth::PriceIdentifier,
    time_chunk::TimeChunkConfiguration,
    Decimal, Nanoseconds,
};
use templar_gateway_client::Network;

use oracle::AssetSpec;
use serde_util::duration;

/// The proxy oracle serves exactly one market, so its two price identifiers are
/// per-oracle constants rather than configuration. They were previously typed
/// out as `cccc…`/`bbbb…` in two files each, with nothing checking they matched.
pub const COLLATERAL_PRICE_ID: PriceIdentifier = PriceIdentifier([0xcc; 32]);
pub const BORROW_PRICE_ID: PriceIdentifier = PriceIdentifier([0xbb; 32]);

/// Default band for the reference-price cross-check.
///
/// Defaulted rather than required so `market export`, which cannot recover a
/// judgement call from chain state, does not have to invent one silently.
pub fn default_reference_tolerance() -> Decimal {
    templar_common::dec!("0.015")
}

/// Bumped only on a breaking spec change; unknown versions are rejected.
///
/// Every struct here is `deny_unknown_fields`, so *adding* a field is breaking
/// in the reader direction: an older build rejects a document carrying it.
///
/// 2: `symbol` became optional, so `market export` can leave it unset rather
///    than invent a ticker.
/// 3: `reference` and `reference_tolerance` on each asset, and
///    `market.reference_tolerance`, for the third-party cross-check. Export
///    always emits the market tolerance, so every exported spec carries it.
pub const SCHEMA_VERSION: u32 = 4;

/// A complete market deployment: the market contract, its dedicated proxy
/// oracle, and the governance contract that owns that oracle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarketSpec {
    pub schema: u32,

    /// Profiles merged beneath this file, in order. Resolved and emptied by
    /// [`extends::load`] before deserialization, so a loaded spec always has an
    /// empty list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<std::path::PathBuf>,

    /// Registry that owns the deployment; every account id derives from it.
    pub registry: AccountId,

    /// Market subaccount label, e.g. `iethfxrp-ixlmusdc`.
    pub name: String,

    pub versions: Versions,

    /// Which oracle the market reads, and therefore what a deployment creates.
    #[serde(default)]
    pub oracle: OracleMode,

    pub governance: GovernanceSpec,

    pub collateral: AssetSpec<CollateralAsset>,
    pub borrow: AssetSpec<BorrowAsset>,
    pub market: MarketParams,
}

/// Which oracle a market reads.
///
/// `Proxy` deploys a dedicated proxy oracle and a governance contract to own
/// it, aggregating the sources each asset names. `Direct` points the market at
/// an oracle that already exists — 16 of the 18 alpha markets do this, reading
/// `pyth-oracle.near`, the LST oracle, or a proxy someone else deployed — so a
/// deployment creates only the market, and each asset names the oracle's own
/// price identifier instead of sources.
///
/// A mode rather than optional fields on one shape, so a spec cannot
/// half-describe either: a direct market has no aggregator to configure, and a
/// proxy market has no external identifier to name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum OracleMode {
    /// Deploy a dedicated proxy oracle for this market.
    #[default]
    Proxy,
    /// Read an oracle that already exists.
    Direct { account_id: AccountId },
}

impl OracleMode {
    /// The oracle the market points at, given the proxy this spec would
    /// otherwise create.
    #[must_use]
    pub fn account_id(&self, proxy_id: AccountId) -> AccountId {
        match self {
            Self::Proxy => proxy_id,
            Self::Direct { account_id } => account_id.clone(),
        }
    }

    #[must_use]
    pub const fn is_direct(&self) -> bool {
        matches!(self, Self::Direct { .. })
    }
}

/// Registry version keys for the three contracts a deployment creates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Versions {
    pub market: String,
    pub proxy_oracle: String,
    pub proxy_governance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GovernanceSpec {
    /// Account allowed to create and execute proposals against the oracle.
    pub admin: AccountId,

    /// Default proposal TTL. `0s` makes every proposal executable on creation,
    /// which is what lets a deploy configure both feeds in single calls.
    #[serde(with = "duration")]
    #[schemars(with = "String")]
    pub ttl_default: Nanoseconds,
}

/// [`MarketConfiguration`] minus everything this module derives.
///
/// The on-chain types are embedded rather than re-modeled, so a field added
/// there surfaces here as a compile error instead of silently going unset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarketParams {
    #[serde(with = "duration")]
    #[schemars(with = "String")]
    pub time_chunk: Nanoseconds,

    /// Maximum price age the market accepts, and the default for each proxy's
    /// `max_age`, so market-side and proxy-side staleness cannot diverge.
    /// Clock drift is bounded separately — see
    /// [`oracle::DEFAULT_MAX_CLOCK_DRIFT`].
    #[serde(with = "duration")]
    #[schemars(with = "String")]
    pub price_maximum_age: Nanoseconds,

    #[schemars(with = "String")]
    pub mcr_maintenance: Decimal,
    #[schemars(with = "String")]
    pub mcr_liquidation: Decimal,
    #[schemars(with = "String")]
    pub maximum_usage_ratio: Decimal,
    #[schemars(with = "String")]
    pub liquidation_maximum_spread: Decimal,

    /// Default band for the reference-price cross-check, as a fraction — `0.015`
    /// is 1.5%. Per-asset `reference_tolerance` overrides it.
    #[serde(default = "default_reference_tolerance")]
    #[schemars(with = "String")]
    pub reference_tolerance: Decimal,

    // These four are embedded on-chain types, which is the point — a field added
    // to `MarketConfiguration` surfaces here as a compile error. None of them
    // implement `JsonSchema` though, so the emitted schema describes them only
    // as "some JSON". Tightening that means deriving `JsonSchema` across the
    // `templar-common` market types, which are compiled into contracts; that is
    // its own piece of work, not ENG-539's.
    #[schemars(with = "serde_json::Value")]
    pub interest_rate_strategy: InterestRateStrategy,
    #[schemars(with = "serde_json::Value")]
    pub origination_fee: Fee<BorrowAsset>,
    #[schemars(with = "serde_json::Value")]
    pub supply_withdrawal_fee: TimeBasedFee<BorrowAsset>,
    #[schemars(with = "serde_json::Value")]
    pub yield_weights: YieldWeights,

    pub protocol_account_id: AccountId,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub borrow_maximum_duration_ms: Option<u64>,

    #[schemars(with = "serde_json::Value")]
    pub borrow_range: AmountRange<BorrowAsset>,
    #[schemars(with = "serde_json::Value")]
    pub supply_range: AmountRange<BorrowAsset>,
    #[schemars(with = "serde_json::Value")]
    pub supply_withdrawal_range: AmountRange<BorrowAsset>,
}

/// The account ids a deployment creates, derived from the market's name and
/// registry.
///
/// Free functions, not just methods: `market export` derives them *before* it
/// has a spec to call methods on, and a second hand-rolled copy of the naming
/// convention is exactly the duplication this whole epic exists to remove.
pub fn market_account_id(name: &str, registry: &AccountId) -> anyhow::Result<AccountId> {
    derived_id(name, registry)
}

pub fn oracle_account_id(name: &str, registry: &AccountId) -> anyhow::Result<AccountId> {
    derived_id(&oracle_name(name), registry)
}

pub fn governance_account_id(name: &str, registry: &AccountId) -> anyhow::Result<AccountId> {
    derived_id(&governance_name(name), registry)
}

/// The sub-account *label* a registry deploy creates, as distinct from the full
/// account id. `registry.deploy` takes the label and derives the id itself, so
/// both forms are needed — and deriving one from the other by string surgery is
/// how the two drifted apart before.
pub fn oracle_name(name: &str) -> String {
    format!("proxy-oracle-{name}")
}

pub fn governance_name(name: &str) -> String {
    format!("proxy-gov-{name}")
}

fn derived_id(label: &str, registry: &AccountId) -> anyhow::Result<AccountId> {
    format!("{label}.{registry}")
        .parse()
        .with_context(|| format!("`{label}.{registry}` is not a valid account id"))
}

/// Convert a spec duration to the coarser unit the chain stores it in, refusing
/// anything that would not survive the conversion.
///
/// The spec accepts any unit `parse_duration` does, but the chain stores
/// `price_maximum_age` in whole seconds and `time_chunk` in whole milliseconds.
/// Dividing silently would make `price_maximum_age = "1500ms"` mean 1s to the
/// market while the proxy still enforced 1.5s, and `time_chunk = "500us"`
/// collapse to a zero-length chunk.
fn exact_units(value: Nanoseconds, per_unit: u64, field: &str, unit: &str) -> anyhow::Result<u64> {
    let ns = value.as_ns();
    anyhow::ensure!(
        ns % per_unit == 0,
        "`{field}` must be a whole number of {unit}; {ns}ns is not, and the chain \
         would store a different value than the proxy enforces"
    );
    Ok(ns / per_unit)
}

/// Convert an authored range through the validating `TryFrom`.
///
/// The spec holds `AmountRange` rather than `ValidAmountRange` because
/// deserializing the latter directly would bypass the min/max invariant that
/// conversion enforces.
fn into_valid<A: templar_common::asset::AssetClass + PartialOrd>(
    range: AmountRange<A>,
    field: &str,
) -> anyhow::Result<ValidAmountRange<A>> {
    ValidAmountRange::try_from(range)
        .with_context(|| format!("invalid `{field}`: maximum must not be below minimum"))
}

impl MarketSpec {
    /// The network this spec targets, derived from the registry's top-level
    /// account rather than declared.
    ///
    /// A spec is bound to one chain by its account ids — `templar-alpha.near`
    /// exists only on mainnet — so carrying a `network` field alongside them
    /// would just be a second place for the same fact to be wrong. `plan`
    /// cross-checks this against `--network`.
    pub fn network(&self) -> anyhow::Result<Network> {
        match self.registry.as_str().rsplit('.').next() {
            Some("near") => Ok(Network::Mainnet),
            Some("testnet") => Ok(Network::Testnet),
            _ => anyhow::bail!(
                "cannot tell which network `{}` belongs to: expected a `.near` or `.testnet` registry",
                self.registry
            ),
        }
    }

    /// `<name>.<registry>` — where the market contract lands.
    pub fn market_id(&self) -> anyhow::Result<AccountId> {
        market_account_id(&self.name, &self.registry)
    }

    /// `proxy-oracle-<name>.<registry>` — the market's dedicated oracle.
    pub fn oracle_id(&self) -> anyhow::Result<AccountId> {
        oracle_account_id(&self.name, &self.registry)
    }

    /// `proxy-gov-<name>.<registry>` — owns the oracle, so it must be deployed
    /// before the oracle names it at init.
    pub fn governance_id(&self) -> anyhow::Result<AccountId> {
        governance_account_id(&self.name, &self.registry)
    }

    /// The price identifiers the market will use.
    ///
    /// A proxy oracle serves the two constants this tool owns, so a proxy spec
    /// need not state them. A pre-existing oracle serves whatever identifiers it
    /// was given, so a direct spec must name them: there is nothing to derive
    /// them from, and guessing would point the market at someone else's feed.
    pub fn price_identifiers(&self) -> anyhow::Result<(PriceIdentifier, PriceIdentifier)> {
        if !self.oracle.is_direct() {
            return Ok((COLLATERAL_PRICE_ID, BORROW_PRICE_ID));
        }
        let named = |side: &str, id: Option<PriceIdentifier>| {
            id.with_context(|| {
                format!(
                    "`{side}.price_id` is required when `oracle` is `direct`: a \
                     pre-existing oracle serves its own identifiers, and this \
                     spec has nothing to derive one from"
                )
            })
        };
        Ok((
            named("collateral", self.collateral.price_id)?,
            named("borrow", self.borrow.price_id)?,
        ))
    }

    /// Build the on-chain market configuration.
    ///
    /// Decimals are supplied by the caller because resolving them reads the
    /// token's metadata on chain (ENG-541); this module stays offline. The
    /// spec's own `decimals` override, when present, is what that lookup
    /// reconciles against.
    pub fn into_market_configuration(
        self,
        collateral_decimals: i32,
        borrow_decimals: i32,
    ) -> anyhow::Result<MarketConfiguration> {
        let oracle_id = self.oracle.account_id(self.oracle_id()?);
        let (collateral_price_id, borrow_price_id) = self.price_identifiers()?;
        let price_maximum_age_s = u32::try_from(exact_units(
            self.market.price_maximum_age,
            1_000_000_000,
            "price_maximum_age",
            "seconds",
        )?)
        .context("`price_maximum_age` exceeds u32 seconds")?;
        let time_chunk_ms = exact_units(
            self.market.time_chunk,
            1_000_000,
            "time_chunk",
            "milliseconds",
        )?;
        anyhow::ensure!(
            time_chunk_ms > 0,
            "`time_chunk` must be at least 1ms; a zero-length chunk has no snapshot schedule"
        );

        Ok(MarketConfiguration {
            time_chunk_configuration: TimeChunkConfiguration::new(time_chunk_ms),
            borrow_asset: self.borrow.asset,
            collateral_asset: self.collateral.asset,
            price_oracle_configuration: PriceOracleConfiguration {
                account_id: oracle_id,
                collateral_asset_price_id: collateral_price_id,
                collateral_asset_decimals: collateral_decimals,
                borrow_asset_price_id: borrow_price_id,
                borrow_asset_decimals: borrow_decimals,
                price_maximum_age_s,
            },
            borrow_mcr_maintenance: self.market.mcr_maintenance,
            borrow_mcr_liquidation: self.market.mcr_liquidation,
            borrow_asset_maximum_usage_ratio: self.market.maximum_usage_ratio,
            borrow_origination_fee: self.market.origination_fee,
            borrow_interest_rate_strategy: self.market.interest_rate_strategy,
            borrow_maximum_duration_ms: self.market.borrow_maximum_duration_ms.map(Into::into),
            borrow_range: into_valid(self.market.borrow_range, "borrow_range")?,
            supply_range: into_valid(self.market.supply_range, "supply_range")?,
            supply_withdrawal_range: into_valid(
                self.market.supply_withdrawal_range,
                "supply_withdrawal_range",
            )?,
            supply_withdrawal_fee: self.market.supply_withdrawal_fee,
            yield_weights: self.market.yield_weights,
            protocol_account_id: self.market.protocol_account_id,
            liquidation_maximum_spread: self.market.liquidation_maximum_spread,
        })
    }
}
