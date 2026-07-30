//! The declarative market deployment spec.
//!
//! One TOML file replaces the four that `script/deploy.sh` reads (`env.sh`,
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
pub mod oracle;
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

/// Bumped only on a breaking spec change; unknown versions are rejected.
pub const SCHEMA_VERSION: u32 = 1;

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
    pub governance: GovernanceSpec,

    pub collateral: AssetSpec<CollateralAsset>,
    pub borrow: AssetSpec<BorrowAsset>,
    pub market: MarketParams,
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
/// The on-chain types are embedded rather than re-modelled, so a field added
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
    derived_id(&format!("proxy-oracle-{name}"), registry)
}

pub fn governance_account_id(name: &str, registry: &AccountId) -> anyhow::Result<AccountId> {
    derived_id(&format!("proxy-gov-{name}"), registry)
}

fn derived_id(label: &str, registry: &AccountId) -> anyhow::Result<AccountId> {
    format!("{label}.{registry}")
        .parse()
        .with_context(|| format!("`{label}.{registry}` is not a valid account id"))
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
        let oracle_id = self.oracle_id()?;
        let price_maximum_age_s =
            u32::try_from(self.market.price_maximum_age.as_ns() / 1_000_000_000)
                .context("`price_maximum_age` exceeds u32 seconds")?;
        let time_chunk_ms = self.market.time_chunk.as_ns() / 1_000_000;

        Ok(MarketConfiguration {
            time_chunk_configuration: TimeChunkConfiguration::new(time_chunk_ms),
            borrow_asset: self.borrow.asset,
            collateral_asset: self.collateral.asset,
            price_oracle_configuration: PriceOracleConfiguration {
                account_id: oracle_id,
                collateral_asset_price_id: COLLATERAL_PRICE_ID,
                collateral_asset_decimals: collateral_decimals,
                borrow_asset_price_id: BORROW_PRICE_ID,
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
