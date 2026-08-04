//! The declarative market deployment spec: one TOML file in which every value
//! that must agree is written once, and anything derivable — account ids, price
//! identifiers, proxy freshness bounds — is derived rather than declared.
//!
//! Everything here is offline; building and checking a spec never needs a
//! network.

pub mod amount;
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
    asset::{AssetClass, BorrowAsset, CollateralAsset},
    interest_rate_strategy::InterestRateStrategy,
    market::{
        AmountRange, MarketConfiguration, PriceOracleConfiguration, ValidAmountRange, YieldWeights,
    },
    oracle::pyth::PriceIdentifier,
    time_chunk::TimeChunkConfiguration,
    Decimal, Nanoseconds,
};
use templar_gateway_client::Network;

use amount::{FeeSpec, TimeBasedFeeSpec};
use oracle::AssetSpec;
use serde_util::duration;

/// The proxy oracle serves exactly one market, so its two price identifiers are
/// per-oracle constants rather than configuration.
pub const COLLATERAL_PRICE_ID: PriceIdentifier = PriceIdentifier([0xcc; 32]);
pub const BORROW_PRICE_ID: PriceIdentifier = PriceIdentifier([0xbb; 32]);

/// Default band for the reference-price cross-check.
///
/// Defaulted rather than required so `market export`, which cannot recover a
/// judgement call from chain state, does not have to invent one silently.
pub fn default_reference_tolerance() -> Decimal {
    templar_common::dec!("0.015")
}

/// Bumped on a breaking spec change; unknown versions are rejected. Every
/// struct here is `deny_unknown_fields`, so adding a field is breaking in the
/// reader direction: an older build rejects a document carrying it.
pub const SCHEMA_VERSION: u32 = 5;

/// A complete market deployment: the market contract, its dedicated proxy
/// oracle, and the governance contract that owns that oracle.
///
/// The parsed form. A spec *file* states `versions` and `[governance]` flat
/// beside `oracle`, because profile composition merges tables and cannot merge
/// enum variants; [`RawMarketSpec`] is that shape, and the conversion between
/// them is where "a proxy states its governance" stops being a rule to check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawMarketSpec", into = "RawMarketSpec")]
pub struct MarketSpec {
    pub schema: u32,

    /// Profiles merged beneath this file, in order. Resolved and emptied by
    /// [`extends::load`] before deserialization, so a loaded spec always has an
    /// empty list.
    pub extends: Vec<std::path::PathBuf>,

    /// Registry that owns the deployment; every account id derives from it.
    pub registry: AccountId,

    /// Market subaccount label, e.g. `iethfxrp-ixlmusdc`.
    pub name: String,

    /// Registry version key for the market contract, which both modes deploy.
    pub market_version: String,

    /// Which oracle the market reads, and therefore what a deployment creates.
    pub oracle: OracleMode,

    pub collateral: AssetSpec<CollateralAsset>,
    pub borrow: AssetSpec<BorrowAsset>,
    pub market: MarketParams,
}

/// Which oracle a market reads: a dedicated proxy this deployment creates, or
/// an existing account whose own price identifiers each asset then names.
///
/// `Proxy` carries what only a proxy deployment has, so a spec that deploys one
/// without saying who governs it, or which versions to deploy, does not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OracleMode {
    Proxy {
        governance: GovernanceSpec,
        /// Registry version keys for the two contracts a proxy deployment adds.
        oracle_version: String,
        governance_version: String,
    },
    Direct {
        account_id: AccountId,
    },
}

impl OracleMode {
    #[must_use]
    pub const fn is_direct(&self) -> bool {
        matches!(self, Self::Direct { .. })
    }
}

/// The shape a spec file is written in: `versions` and `[governance]` sit flat
/// beside `oracle`, so a profile can supply them and a market override one.
///
/// In proxy mode — the default — `governance`, `versions.proxy_oracle` and
/// `versions.proxy_governance` are all required, and a spec omitting any of them
/// is refused when it is parsed into a `MarketSpec`. They are optional here
/// because a direct market inherits them from a shared profile and ignores them.
/// That conditional is stated rather than encoded: this type's generated schema
/// describes the fields, not the rule relating them.
///
/// Only [`MarketSpec`]'s serde impls construct this. `extends::load` merges at
/// this level, which is why the variant-carried form cannot be the file shape —
/// merging `[oracle.proxy]` from a profile with `[oracle.direct]` from a market
/// yields a table that is both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawMarketSpec {
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<std::path::PathBuf>,
    pub registry: AccountId,
    pub name: String,
    pub versions: Versions,
    #[serde(default)]
    pub oracle: RawOracleMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governance: Option<GovernanceSpec>,
    pub collateral: AssetSpec<CollateralAsset>,
    pub borrow: AssetSpec<BorrowAsset>,
    pub market: MarketParams,
}

/// `oracle = "proxy"` or `[oracle.direct]`, with no payload — the payload lives
/// in sibling tables so profiles can compose it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RawOracleMode {
    #[default]
    Proxy,
    Direct {
        account_id: AccountId,
    },
}

/// Registry version keys, as written. The proxy keys are optional here and
/// required by the conversion into [`MarketSpec`], which is the only place that
/// distinction exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Versions {
    pub market: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_oracle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_governance: Option<String>,
}

impl TryFrom<RawMarketSpec> for MarketSpec {
    type Error = anyhow::Error;

    fn try_from(raw: RawMarketSpec) -> anyhow::Result<Self> {
        let oracle = match raw.oracle {
            // A direct market drops the proxy fields rather than refusing them:
            // the shared mainnet profiles state `[governance]` and both proxy
            // versions for the proxy markets, and every direct market inherits
            // them without asking.
            RawOracleMode::Direct { account_id } => OracleMode::Direct { account_id },
            RawOracleMode::Proxy => OracleMode::Proxy {
                governance: raw.governance.context(
                    "this spec deploys its own proxy oracle but states no \
                     `[governance]`; the oracle would have no owner able to \
                     configure it",
                )?,
                oracle_version: raw
                    .versions
                    .proxy_oracle
                    .context("this spec deploys its own proxy oracle but states no `versions.proxy_oracle`")?,
                governance_version: raw.versions.proxy_governance.context(
                    "this spec deploys its own proxy oracle but states no `versions.proxy_governance`",
                )?,
            },
        };

        Ok(Self {
            schema: raw.schema,
            extends: raw.extends,
            registry: raw.registry,
            name: raw.name,
            market_version: raw.versions.market,
            oracle,
            collateral: raw.collateral,
            borrow: raw.borrow,
            market: raw.market,
        })
    }
}

impl From<MarketSpec> for RawMarketSpec {
    fn from(spec: MarketSpec) -> Self {
        let (oracle, governance, proxy_oracle, proxy_governance) = match spec.oracle {
            OracleMode::Direct { account_id } => {
                (RawOracleMode::Direct { account_id }, None, None, None)
            }
            OracleMode::Proxy {
                governance,
                oracle_version,
                governance_version,
            } => (
                RawOracleMode::Proxy,
                Some(governance),
                Some(oracle_version),
                Some(governance_version),
            ),
        };

        Self {
            schema: spec.schema,
            extends: spec.extends,
            registry: spec.registry,
            name: spec.name,
            versions: Versions {
                market: spec.market_version,
                proxy_oracle,
                proxy_governance,
            },
            oracle,
            governance,
            collateral: spec.collateral,
            borrow: spec.borrow,
            market: spec.market,
        }
    }
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
/// there surfaces here as a compile error instead of silently going unset. The
/// exception is anything holding a borrow-denominated amount, which must carry
/// its unit to be authorable: those keep the property in
/// [`MarketSpec::into_market_configuration`], which builds the on-chain types
/// by struct literal and exhaustive match.
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

    // Embedded on-chain types: a field added to `MarketConfiguration` surfaces
    // here as a compile error. None implement `JsonSchema`, so the emitted
    // schema describes them only as "some JSON".
    #[schemars(with = "serde_json::Value")]
    pub interest_rate_strategy: InterestRateStrategy,
    #[schemars(with = "serde_json::Value")]
    pub yield_weights: YieldWeights,

    pub protocol_account_id: AccountId,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub borrow_maximum_duration_ms: Option<u64>,

    // Borrow-denominated, so every amount states its unit.
    pub origination_fee: FeeSpec,
    pub supply_withdrawal_fee: TimeBasedFeeSpec,
    pub borrow_range: amount::Range,
    pub supply_range: amount::Range,
    pub supply_withdrawal_range: amount::Range,
}

impl MarketParams {
    /// Every amount this states, in the order a reader meets them.
    fn amounts(&self) -> impl Iterator<Item = amount::Amount> + '_ {
        self.origination_fee
            .amount()
            .into_iter()
            .chain(self.supply_withdrawal_fee.fee.amount())
            .chain(self.borrow_range.amounts())
            .chain(self.supply_range.amounts())
            .chain(self.supply_withdrawal_range.amounts())
    }

    /// A decimals value that stands in for an unresolved one, or `None` when no
    /// value can.
    ///
    /// Scaling every borrow-denominated amount by one factor preserves the
    /// orderings between them, which is all [`MarketConfiguration::validate`]
    /// compares — but only while they share a unit. An `atoms` amount is the
    /// same number at every decimals and a `tokens` amount is not, so a spec
    /// mixing the two orders differently under any stand-in than it will on
    /// chain: `500000 atoms` sits above `0.6 tokens` at one decimal and below it
    /// at six.
    pub fn stand_in_borrow_decimals(&self) -> Option<u8> {
        let (mut any_atoms, mut deepest) = (false, None);
        for amount in self.amounts() {
            match amount {
                // Zero is zero at every decimals, so it orders identically under
                // any stand-in whichever unit it is written in. Every spec keeps
                // its fees at `0 atoms`, which would otherwise mix them all.
                amount::Amount::Atoms(0) => {}
                amount::Amount::Atoms(_) => any_atoms = true,
                amount::Amount::Tokens { scale, .. } => {
                    deepest = Some(deepest.unwrap_or(0).max(scale));
                }
            }
        }
        match (any_atoms, deepest) {
            (true, Some(_)) => None,
            // All `atoms` convert to themselves at every decimals, so anything
            // orders them the way the chain will.
            (_, deepest) => Some(deepest.unwrap_or_default()),
        }
    }
}

/// A free function because `market export` derives this before it has a spec to
/// call methods on.
pub fn governance_account_id(name: &str, registry: &AccountId) -> anyhow::Result<AccountId> {
    derived_id(&governance_name(name), registry)
}

/// The sub-account *label* a registry deploy creates, as distinct from the full
/// account id. `registry.deploy` takes the label and derives the id itself, so
/// both forms are needed.
pub fn oracle_name(name: &str) -> String {
    format!("proxy-oracle-{name}")
}

pub fn governance_name(name: &str) -> String {
    format!("proxy-gov-{name}")
}

/// Wall-clock since the Unix epoch. The one clock this tool reads: the oracle
/// kernel takes `now` explicitly rather than reading one, which is what makes it
/// testable, so every caller here gets its reading from the same place.
pub fn wall_clock() -> Nanoseconds {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Nanoseconds::from_ns(u64::try_from(since_epoch.as_nanos()).unwrap_or(u64::MAX))
}

fn derived_id(label: &str, registry: &AccountId) -> anyhow::Result<AccountId> {
    // A dotted label parses as a valid account id but is not a *direct* child,
    // which is what `registry.deploy` requires — so it would pass every derived
    // id and fail on chain with the deposit already attached.
    anyhow::ensure!(
        !label.contains('.'),
        "`{label}` is not a single account label; a registry deploys only its \
         direct sub-accounts, so `name` cannot contain a dot"
    );
    format!("{label}.{registry}")
        .parse()
        .with_context(|| format!("`{label}.{registry}` is not a valid account id"))
}

/// Convert a spec duration to the coarser unit the chain stores it in, refusing
/// what would not survive: dividing silently would make `"1500ms"` mean 1s to
/// the market while the proxy still enforced 1.5s.
fn exact_units(value: Nanoseconds, per_unit: u64, field: &str, unit: &str) -> anyhow::Result<u64> {
    let ns = value.as_ns();
    anyhow::ensure!(
        ns % per_unit == 0,
        "`{field}` must be a whole number of {unit}; {ns}ns is not, and the chain \
         would store a different value than the proxy enforces"
    );
    Ok(ns / per_unit)
}

/// Scale an authored range to base units, then through the validating
/// `TryFrom`.
///
/// The spec holds unit-carrying amounts rather than `ValidAmountRange` because
/// deserializing the latter directly would bypass both the unit and the min/max
/// invariant that conversion enforces.
fn into_valid<A: AssetClass + PartialOrd>(
    range: amount::Range,
    decimals: u8,
    field: &str,
) -> anyhow::Result<ValidAmountRange<A>> {
    let range: AmountRange<A> = range
        .into_on_chain(decimals)
        .with_context(|| format!("invalid `{field}`"))?;
    ValidAmountRange::try_from(range)
        .with_context(|| format!("invalid `{field}`: maximum must not be below minimum"))
}

impl MarketSpec {
    /// The network this spec targets, derived from the registry rather than
    /// declared: a `network` field would be a second place to be wrong.
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

    /// The governance spec and the two version keys a proxy deployment needs,
    /// or `None` for a direct market that creates neither.
    pub const fn proxy(&self) -> Option<(&GovernanceSpec, &String, &String)> {
        match &self.oracle {
            OracleMode::Direct { .. } => None,
            OracleMode::Proxy {
                governance,
                oracle_version,
                governance_version,
            } => Some((governance, oracle_version, governance_version)),
        }
    }

    /// `<name>.<registry>` — where the market contract lands.
    pub fn market_id(&self) -> anyhow::Result<AccountId> {
        derived_id(&self.name, &self.registry)
    }

    /// `proxy-oracle-<name>.<registry>`, derivable in either mode. Callers
    /// meaning "the oracle this market reads" want [`Self::reads_oracle_id`].
    pub fn oracle_id(&self) -> anyhow::Result<AccountId> {
        derived_id(&oracle_name(&self.name), &self.registry)
    }

    /// The oracle this market reads. Derived lazily: `proxy-oracle-<name>` can
    /// exceed NEAR's length limit for a name that is itself valid.
    pub fn reads_oracle_id(&self) -> anyhow::Result<AccountId> {
        match &self.oracle {
            OracleMode::Direct { account_id } => Ok(account_id.clone()),
            OracleMode::Proxy { .. } => self.oracle_id(),
        }
    }

    /// The proxy oracle this deployment creates, if it creates one.
    pub fn own_proxy_id(&self) -> anyhow::Result<Option<AccountId>> {
        match &self.oracle {
            OracleMode::Direct { .. } => Ok(None),
            OracleMode::Proxy { .. } => self.oracle_id().map(Some),
        }
    }

    /// The governance contract this deployment creates, if it creates one.
    pub fn own_governance_id(&self) -> anyhow::Result<Option<AccountId>> {
        match &self.oracle {
            OracleMode::Direct { .. } => Ok(None),
            OracleMode::Proxy { .. } => self.governance_id().map(Some),
        }
    }

    /// `proxy-gov-<name>.<registry>` — owns the oracle, so it must be deployed
    /// before the oracle names it at init.
    pub fn governance_id(&self) -> anyhow::Result<AccountId> {
        governance_account_id(&self.name, &self.registry)
    }

    /// The price identifiers the market will use. A proxy serves the constants
    /// this tool owns; a direct spec must name its oracle's own, since guessing
    /// would point the market at someone else's feed.
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

    /// Build the on-chain market configuration. Decimals are supplied because
    /// resolving them reads token metadata, and this module stays offline.
    pub fn into_market_configuration(
        self,
        collateral_decimals: i32,
        borrow_decimals: i32,
    ) -> anyhow::Result<MarketConfiguration> {
        let oracle_id = self.reads_oracle_id()?;
        let (collateral_price_id, borrow_price_id) = self.price_identifiers()?;
        // Every amount below is borrow-denominated; nothing here is stated in
        // collateral.
        let amount_decimals = u8::try_from(borrow_decimals)
            .context("the borrow asset's decimals do not fit a u8, so no amount can be scaled")?;
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
        // The contract's own bound, not a rounder one: `now()` is
        // `block_timestamp_ms / duration_ms`, and `Market::new` unwraps
        // `previous()`, so a chunk longer than the deploying block's clock
        // panics at init. Wall-clock stands in for that block, and the bound
        // only ever loosens.
        anyhow::ensure!(
            time_chunk_ms > 0,
            "`time_chunk` must be at least 1ms; a zero-length chunk has no \
             snapshot schedule"
        );
        anyhow::ensure!(
            time_chunk_ms <= wall_clock().as_ms(),
            "`time_chunk` is {time_chunk_ms}ms, longer than the time since the \
             Unix epoch, so the market's first snapshot would precede chunk zero \
             and initialization would panic"
        );

        // `total_weight` panics on overflow and nothing on the deploy path calls
        // it, so an oversized set deploys and then bricks the first supply.
        let weights = &self.market.yield_weights;
        let total = weights
            .r#static
            .values()
            .try_fold(u16::from(weights.supply), |sum, weight| {
                sum.checked_add(*weight)
            });
        anyhow::ensure!(
            total.is_some(),
            "`yield_weights` sum past u16: supply is {} and the static weights add \
             {}. Every yield calculation would panic.",
            weights.supply,
            weights
                .r#static
                .values()
                .map(|w| u32::from(*w))
                .sum::<u32>(),
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
            borrow_origination_fee: self
                .market
                .origination_fee
                .into_on_chain(amount_decimals)
                .context("invalid `origination_fee`")?,
            borrow_interest_rate_strategy: self.market.interest_rate_strategy,
            borrow_maximum_duration_ms: self.market.borrow_maximum_duration_ms.map(Into::into),
            borrow_range: into_valid(self.market.borrow_range, amount_decimals, "borrow_range")?,
            supply_range: into_valid(self.market.supply_range, amount_decimals, "supply_range")?,
            supply_withdrawal_range: into_valid(
                self.market.supply_withdrawal_range,
                amount_decimals,
                "supply_withdrawal_range",
            )?,
            supply_withdrawal_fee: self
                .market
                .supply_withdrawal_fee
                .into_on_chain(amount_decimals)
                .context("invalid `supply_withdrawal_fee`")?,
            yield_weights: self.market.yield_weights,
            protocol_account_id: self.market.protocol_account_id,
            liquidation_maximum_spread: self.market.liquidation_maximum_spread,
        })
    }
}
