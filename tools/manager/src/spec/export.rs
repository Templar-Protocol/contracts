//! The inverse of [`super::MarketSpec::into_market_configuration`]: reconstruct
//! a spec from what is actually deployed.
//!
//! This is the epic's validation gate. The tool must reproduce markets we
//! already run before it is trusted to create new ones.
//!
//! **Scope is deliberately narrow.** A spec expresses only proxy-oracle
//! deployments — a dedicated oracle at `proxy-oracle-<market-name>.<registry>`
//! with the [`COLLATERAL_PRICE_ID`]/[`BORROW_PRICE_ID`] constants. Of the alpha
//! markets, most predate that: thirteen read `pyth-oracle.near` directly, two
//! use a proxy named after the *asset pair* rather than the market, and one uses
//! the LST oracle. Every one of those is **refused, not approximated** — a spec
//! that re-derives to a different oracle account would be a silent wrong answer,
//! and worse than no answer.

use anyhow::Context as _;
use near_account_id::AccountId;
use templar_common::{
    asset::AssetClass,
    market::{MarketConfiguration, PriceOracleConfiguration},
    Nanoseconds,
};
use templar_proxy_oracle_kernel::proxy::{aggregator::Aggregator, Proxy};
use templar_proxy_oracle_near_common::{
    input::Source, price_transformer::Action, request::OracleRequest,
};

use super::{
    oracle::{AggregatorSpec, AssetSpec, SourceSpec, LST_CALL_GAS},
    GovernanceSpec, MarketParams, MarketSpec, Versions, BORROW_PRICE_ID, COLLATERAL_PRICE_ID,
    SCHEMA_VERSION,
};

/// Everything read off chain for one market.
///
/// The proxy and governance fields are `None` for a direct market, which reads
/// an oracle it does not own: there is no proxy of ours to fetch, and the
/// governance account beside it was never deployed.
pub struct Deployed {
    pub market_id: AccountId,
    pub configuration: MarketConfiguration,
    pub collateral_proxy: Option<Proxy<Source>>,
    pub borrow_proxy: Option<Proxy<Source>>,
    pub versions: Versions,
    pub governance: Option<GovernanceSpec>,
}

impl MarketSpec {
    /// Reconstruct a spec, or explain why this market cannot be expressed as one.
    pub fn from_deployed(deployed: Deployed) -> anyhow::Result<Self> {
        let (name, registry) = split_market_id(&deployed.market_id)?;
        let oracle = &deployed.configuration.price_oracle_configuration;

        // A market whose proxies were not read is one that reads someone else's
        // oracle. It names that account and each asset's identifier on it,
        // rather than being refused as inexpressible.
        let direct = deployed.collateral_proxy.is_none() && deployed.borrow_proxy.is_none();

        let spec = Self {
            oracle: if direct {
                super::OracleMode::Direct {
                    account_id: oracle.account_id.clone(),
                }
            } else {
                super::OracleMode::Proxy
            },
            schema: SCHEMA_VERSION,
            extends: Vec::new(),
            registry,
            name,
            versions: deployed.versions,
            governance: deployed.governance,
            collateral: asset_spec(
                "collateral",
                deployed.configuration.collateral_asset.clone(),
                oracle.collateral_asset_decimals,
                deployed.collateral_proxy,
                Some(oracle.collateral_asset_price_id),
            )?,
            borrow: asset_spec(
                "borrow",
                deployed.configuration.borrow_asset.clone(),
                oracle.borrow_asset_decimals,
                deployed.borrow_proxy,
                Some(oracle.borrow_asset_price_id),
            )?,
            market: market_params(&deployed.configuration),
        };
        // Built before the oracle check so the error can name the derived id,
        // which needs a complete spec. Freshness bounds come straight from the
        // deployed proxies rather than being left to defaults: an export exists
        // for fidelity, and a later change to a default must not silently alter
        // what an exported spec means.
        if !direct {
            ensure_expressible(&spec, oracle)?;
        }
        Ok(spec)
    }
}

/// `<name>.<registry>` — the inverse of [`MarketSpec::market_id`].
pub fn split_market_id(market_id: &AccountId) -> anyhow::Result<(String, AccountId)> {
    let registry = market_id.get_parent_account_id().with_context(|| {
        format!("`{market_id}` is a top-level account, not a market under a registry")
    })?;
    let name = market_id
        .as_str()
        .strip_suffix(&format!(".{registry}"))
        .with_context(|| format!("`{market_id}` does not end with `.{registry}`"))?;
    anyhow::ensure!(
        !name.is_empty(),
        "`{market_id}` has an empty market name, so every derived account id \
         would be malformed"
    );
    Ok((name.to_owned(), registry.to_owned()))
}

/// Refuse anything the spec cannot round-trip faithfully.
fn ensure_expressible(spec: &MarketSpec, oracle: &PriceOracleConfiguration) -> anyhow::Result<()> {
    let derived = spec.oracle_id()?;
    anyhow::ensure!(
        derived == oracle.account_id,
        "market reads `{}`, but a spec derives `{derived}`. Only markets with a \
         dedicated proxy oracle named after them can be expressed as a spec; \
         this one cannot be exported.",
        oracle.account_id,
    );
    anyhow::ensure!(
        oracle.collateral_asset_price_id == COLLATERAL_PRICE_ID
            && oracle.borrow_asset_price_id == BORROW_PRICE_ID,
        "oracle `{}` uses price ids a spec does not derive ({} / {}). Only the \
         per-market constants are expressible; this market cannot be exported.",
        oracle.account_id,
        hex::encode(oracle.collateral_asset_price_id.0),
        hex::encode(oracle.borrow_asset_price_id.0),
    );
    Ok(())
}

fn asset_spec<A: AssetClass>(
    side: &str,
    asset: templar_common::asset::FungibleAsset<A>,
    decimals: i32,
    proxy: Option<Proxy<Source>>,
    price_id: Option<templar_common::oracle::pyth::PriceIdentifier>,
) -> anyhow::Result<AssetSpec<A>> {
    let decimals = Some(u8::try_from(decimals).with_context(|| {
        format!("{side} asset declares {decimals} decimals, which a spec cannot express")
    })?);

    // A direct market names the identifier its oracle serves this asset under,
    // and aggregates nothing.
    let Some(proxy) = proxy else {
        return Ok(AssetSpec {
            asset,
            price_id,
            symbol: None,
            reference: None,
            reference_tolerance: None,
            decimals,
            aggregator: None,
            min_sources: 0,
            sources: Vec::new(),
            max_age: None,
            max_clock_drift: None,
        });
    };

    let (aggregator, min_sources, sources) = split_aggregator(proxy.aggregator)?;

    // `None` means opposite things on the two sides of this conversion. On chain
    // it is *unbounded* — `FreshnessFilter::accepts` admits a price of any age.
    // In a spec it means *unspecified*, and `AssetSpec::into_proxy` fills it from
    // `price_maximum_age` / `DEFAULT_MAX_CLOCK_DRIFT`. Copying it through would
    // turn "accept any age" into "enforce the market's bound" on the next
    // deploy, silently and with no diff to notice.
    let freshness = &proxy.freshness_filter;
    anyhow::ensure!(
        freshness.max_age_ns.is_some() && freshness.max_clock_drift_ns.is_some(),
        "the {side} proxy leaves a freshness bound unset, which is unbounded on \
         chain but means `use the default` in a spec. Re-deploying an exported \
         spec would silently start enforcing a bound this oracle does not have, \
         so this market cannot be exported."
    );

    Ok(AssetSpec {
        // A proxy serves the constants this tool owns; there is no external
        // identifier to recover.
        price_id: None,
        asset,
        // Never reaches the chain, so it cannot be recovered. Left unset for a
        // human to fill in; inventing a plausible ticker would be worse, since
        // the reference cross-check (ENG-543) would then verify a guess.
        symbol: None,
        // Neither reaches the chain. A pinned reference id is an assertion about
        // what a wrapped token tracks; inventing one would be a guess presented
        // as a record.
        reference: None,
        reference_tolerance: None,
        decimals,
        aggregator: Some(aggregator),
        min_sources,
        sources,
        max_age: freshness.max_age_ns,
        max_clock_drift: freshness.max_clock_drift_ns,
    })
}

/// One on-chain source as a spec source. `weight` is `None` for `priority`,
/// which carries none.
fn source_spec(source: Source, weight: Option<u32>) -> anyhow::Result<SourceSpec> {
    let request = match source {
        Source::Request(request) => request,
        Source::Transformer(transformer) => return lst_spec(transformer, weight),
    };
    Ok(match request {
        OracleRequest::Lazer(lazer) => SourceSpec::Lazer {
            oracle: lazer.oracle_id,
            feed_id: lazer.feed_id,
            weight,
        },
        OracleRequest::RedStone(redstone) => SourceSpec::RedStone {
            oracle: redstone.oracle_id,
            price_id: redstone.price_id.to_string(),
            weight,
        },
        OracleRequest::Pyth(pyth) => SourceSpec::Pyth {
            oracle: pyth.oracle_id,
            price_id: pyth.price_id,
            weight,
        },
    })
}

/// A price-transformer source as `SourceSpec::Lst`.
///
/// The spec fixes the view's arguments and gas, so anything else is refused
/// rather than exported into a spec that would redeploy with different ones.
fn lst_spec(
    transformer: templar_proxy_oracle_near_common::input::ProxyPriceTransformer,
    weight: Option<u32>,
) -> anyhow::Result<SourceSpec> {
    let OracleRequest::Pyth(pyth) = transformer.request else {
        anyhow::bail!("a price transformer over a non-Pyth source is not expressible as a spec");
    };
    let Action::NormalizeNativeLstPrice { decimals } = transformer.action;

    anyhow::ensure!(
        transformer.call.args.0 == b"null",
        "the transformer calls `{}` with arguments; a spec's LST source calls a \
         no-argument view",
        transformer.call.method_name,
    );
    anyhow::ensure!(
        transformer.call.gas.0 == LST_CALL_GAS.as_gas(),
        "the transformer prepays {} gas for `{}`, but a spec's LST source always \
         prepays {}",
        transformer.call.gas.0,
        transformer.call.method_name,
        LST_CALL_GAS.as_gas(),
    );

    Ok(SourceSpec::Lst {
        oracle: pyth.oracle_id,
        price_id: pyth.price_id,
        contract: transformer.call.account_id,
        method: transformer.call.method_name,
        decimals,
        weight,
    })
}

fn split_aggregator(
    aggregator: Aggregator<Source>,
) -> anyhow::Result<(AggregatorSpec, u32, Vec<SourceSpec>)> {
    let (kind, sources, min_sources) = match aggregator {
        Aggregator::MedianLow(median) => (
            AggregatorSpec::MedianLow,
            median.sources,
            median.min_sources,
        ),
        Aggregator::MedianHigh(median) => (
            AggregatorSpec::MedianHigh,
            median.sources,
            median.min_sources,
        ),
        Aggregator::Priority(priority) => {
            return Ok((
                AggregatorSpec::Priority,
                0,
                priority
                    .sources
                    .into_iter()
                    .map(|source| source_spec(source, None))
                    .collect::<anyhow::Result<_>>()?,
            ))
        }
    };

    let sources = sources
        .into_iter()
        .map(|weighted| source_spec(weighted.source, Some(weighted.weight)))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok((kind, min_sources, sources))
}

fn market_params(configuration: &MarketConfiguration) -> MarketParams {
    MarketParams {
        time_chunk: Nanoseconds::from_ns(
            configuration
                .time_chunk_configuration
                .duration_ms()
                .saturating_mul(1_000_000),
        ),
        price_maximum_age: Nanoseconds::from_ns(
            u64::from(configuration.price_oracle_configuration.price_maximum_age_s) * 1_000_000_000,
        ),
        mcr_maintenance: configuration.borrow_mcr_maintenance,
        mcr_liquidation: configuration.borrow_mcr_liquidation,
        maximum_usage_ratio: configuration.borrow_asset_maximum_usage_ratio,
        liquidation_maximum_spread: configuration.liquidation_maximum_spread,
        reference_tolerance: super::default_reference_tolerance(),
        interest_rate_strategy: configuration.borrow_interest_rate_strategy.clone(),
        origination_fee: configuration.borrow_origination_fee.clone(),
        supply_withdrawal_fee: configuration.supply_withdrawal_fee.clone(),
        yield_weights: configuration.yield_weights.clone(),
        protocol_account_id: configuration.protocol_account_id.clone(),
        borrow_maximum_duration_ms: configuration.borrow_maximum_duration_ms.map(|ms| ms.0),
        borrow_range: (*configuration.borrow_range).clone(),
        supply_range: (*configuration.supply_range).clone(),
        supply_withdrawal_range: (*configuration.supply_withdrawal_range).clone(),
    }
}
