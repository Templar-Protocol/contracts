//! Read-only price preflight: verify provider inputs or read stored inputs, then
//! apply the same [`Proxy::resolve`] the contract runs. Stored freshness is
//! reported separately and never substitutes for failed provider verification.

use anyhow::Context as _;
use templar_common::asset::AssetClass;
use templar_common::oracle::{pyth, redstone as redstone_types};
use templar_common::Nanoseconds;
use templar_gateway_core::{DispatchRead, GatewayContext};
use templar_gateway_methods_spec::{contract, redstone};
use templar_gateway_oracle_updates_dispatch::{
    Dispatch as OracleUpdatesDispatch, WithLazerSource, WithRedStoneSource,
};
use templar_gateway_oracle_updates_spec::oracle::{GetLazerUpdate, GetRedStoneUpdate};
use templar_gateway_types::common::ContractArgs;
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{CircuitBreaker, CircuitBreakerSet};
use templar_proxy_oracle_kernel::proxy::freshness_filter::FreshnessFilter;
use templar_proxy_oracle_kernel::Price;
use templar_proxy_oracle_near_common::convert::pyth_price_try_to_kernel;
use templar_proxy_oracle_near_common::price_transformer::Action;

use super::scaled;
use crate::commands::spec::{PreflightPriceArgs, PricesFrom};
use crate::context::{lazer_source, redstone_source, CliContext};
use crate::spec::{
    check::{Check, Status},
    oracle::{AssetSpec, SourceSpec, DEFAULT_MAX_CLOCK_DRIFT},
    MarketSpec, BORROW_PRICE_ID, COLLATERAL_PRICE_ID,
};

/// `oracle.aggregate.{collateral,borrow,pair}`, both legs judged against one
/// wall-clock reading taken after every result arrives — as the contract's
/// callback does. Per-leg clocks would admit a ratio it could never accept.
pub(super) async fn checks(
    ctx: &CliContext,
    spec: &MarketSpec,
    deployed_oracle: Option<&near_account_id::AccountId>,
    args: &PreflightPriceArgs,
) -> (Vec<Check>, Option<Price>, Option<Price>) {
    // Nothing to dry-run for a direct market: this reproduces a *proxy's*
    // aggregation, and an oracle we did not configure has none of ours to
    // reproduce. Reported as not run rather than silently passing. Its prices
    // still reach the reference cross-check — `oracle.serves_pair` reads them.
    if spec.oracle.is_direct() {
        let skipped = Check::new(
            "oracle.aggregate.all",
            Status::Skipped {
                reason: "chain inputs: this market reads an existing oracle; there is no \
                         proxy aggregation to reproduce"
                    .to_owned(),
            },
        );
        return (vec![skipped], None, None);
    }

    let providers = Providers::new(ctx, spec, args);

    // Against the oracle's own breakers when one is deployed. An empty set is
    // right for `market plan` — the oracle does not exist yet — and wrong for
    // `market verify`: a tripped breaker means the live oracle prices nothing,
    // and resolving without it would report the aggregation healthy for a
    // market that is blocked.
    let (collateral, borrow, collateral_breakers, borrow_breakers) = futures::join!(
        fetch_all(ctx, &spec.collateral, &providers, args.prices_from),
        fetch_all(ctx, &spec.borrow, &providers, args.prices_from),
        breakers(ctx, deployed_oracle, COLLATERAL_PRICE_ID),
        breakers(ctx, deployed_oracle, BORROW_PRICE_ID),
    );

    // Sampled after the fetches, not before: a feed updated mid-sweep would
    // otherwise read as future-drifted against a clock taken before it was
    // even requested.
    let now = crate::spec::wall_clock();

    let (collateral_price, mut checks) = leg(
        "collateral",
        &spec.collateral,
        spec,
        collateral,
        now,
        collateral_breakers,
        args.prices_from,
    );
    let (borrow_price, borrow_checks) = leg(
        "borrow",
        &spec.borrow,
        spec,
        borrow,
        now,
        borrow_breakers,
        args.prices_from,
    );
    checks.extend(borrow_checks);
    checks.push(pair(
        collateral_price,
        borrow_price,
        leg_inputs(&spec.collateral, args.prices_from),
        leg_inputs(&spec.borrow, args.prices_from),
    ));
    (checks, collateral_price, borrow_price)
}

/// The oracle's configured breakers for a feed. A failed read is returned
/// rather than treated as an empty set, which trips on nothing and so could
/// only turn a Failed aggregation into a Passed.
async fn breakers(
    ctx: &CliContext,
    oracle_id: Option<&near_account_id::AccountId>,
    id: templar_common::oracle::pyth::PriceIdentifier,
) -> anyhow::Result<CircuitBreakerSet<CircuitBreaker>> {
    let Some(oracle_id) = oracle_id else {
        return Ok(CircuitBreakerSet::empty());
    };
    let result = ctx
        .client
        .read(
            templar_gateway_methods_spec::proxy_oracle::GetProxyCircuitBreakerSet {
                oracle_id: oracle_id.clone(),
                id,
            },
        )
        .await
        .with_context(|| format!("read the circuit-breaker set for {id:?} on {oracle_id}"))?;
    Ok(result.unwrap_or_else(CircuitBreakerSet::empty))
}

type PriceResult = anyhow::Result<Option<Price>>;

/// Construction failures are independent: a missing Lazer key must not hide
/// the RedStone leg, and unused sources must not start a process or need a key.
struct Providers {
    lazer: Option<anyhow::Result<WithLazerSource<GatewayContext>>>,
    redstone: Option<anyhow::Result<WithRedStoneSource<GatewayContext>>>,
}

impl Providers {
    fn new(ctx: &CliContext, spec: &MarketSpec, args: &PreflightPriceArgs) -> Self {
        let mut sources = spec.collateral.sources.iter().chain(&spec.borrow.sources);
        let provider = args.prices_from == PricesFrom::Provider && !spec.oracle.is_direct();
        let lazer = provider
            && sources
                .clone()
                .any(|s| matches!(s, SourceSpec::Lazer { .. }));
        let redstone = provider && sources.any(|s| matches!(s, SourceSpec::RedStone { .. }));
        Self {
            lazer: lazer.then(|| {
                lazer_source(
                    GatewayContext::new(ctx.network_config().clone())?,
                    &args.lazer,
                )
            }),
            redstone: redstone.then(|| {
                redstone_source(
                    GatewayContext::new(ctx.network_config().clone())?,
                    &args.redstone,
                )
            }),
        }
    }

    async fn fetch(&self, source: &SourceSpec) -> PriceResult {
        let price = match source {
            SourceSpec::Lazer {
                oracle, feed_id, ..
            } => {
                let context = self
                    .lazer
                    .as_ref()
                    .context("Lazer source not constructed")?
                    .as_ref()
                    .map_err(|error| anyhow::anyhow!("{error:#}"))?;
                OracleUpdatesDispatch::dispatch(
                    GetLazerUpdate {
                        oracle_id: oracle.clone(),
                        feed_ids: vec![*feed_id],
                    },
                    context.clone(),
                )
                .await?
                .remove(feed_id)
                .flatten()
                .and_then(|feed| feed.to_ema_price())
            }
            SourceSpec::RedStone {
                oracle, price_id, ..
            } => {
                let context = self
                    .redstone
                    .as_ref()
                    .context("RedStone source not constructed")?
                    .as_ref()
                    .map_err(|error| anyhow::anyhow!("{error:#}"))?;
                OracleUpdatesDispatch::dispatch(
                    GetRedStoneUpdate {
                        oracle_id: oracle.clone(),
                        feed_ids: vec![price_id.clone().into()],
                    },
                    context.clone(),
                )
                .await?
                .first()
                .and_then(|entry| entry.data.to_pyth_price())
            }
            SourceSpec::Pyth { .. } | SourceSpec::Lst { .. } => {
                anyhow::bail!("this source is chain-only")
            }
        };
        Ok(price.as_ref().and_then(pyth_price_try_to_kernel))
    }
}

/// Chain-only paths own one result, borrowed for both selected and diagnostic
/// checks. Provider paths cannot accidentally substitute their stored result.
enum SourcePrices {
    Chain(PriceResult),
    Provider {
        selected: PriceResult,
        stored: PriceResult,
    },
}

impl SourcePrices {
    fn selected(&self) -> &PriceResult {
        match self {
            Self::Chain(price) => price,
            Self::Provider { selected, .. } => selected,
        }
    }

    fn stored(&self) -> &PriceResult {
        match self {
            Self::Chain(price) => price,
            Self::Provider { stored, .. } => stored,
        }
    }
}

fn source_inputs(source: &SourceSpec, mode: PricesFrom) -> &'static str {
    match (mode, source) {
        (PricesFrom::Provider, SourceSpec::Lazer { .. } | SourceSpec::RedStone { .. }) => {
            "provider"
        }
        _ => "chain",
    }
}

fn leg_inputs<A: AssetClass>(asset: &AssetSpec<A>, mode: PricesFrom) -> &'static str {
    let provider = asset
        .sources
        .iter()
        .any(|s| source_inputs(s, mode) == "provider");
    let chain = asset
        .sources
        .iter()
        .any(|s| source_inputs(s, mode) == "chain");
    match (provider, chain) {
        (true, true) => "provider and chain",
        (true, false) => "provider",
        _ => "chain",
    }
}

async fn fetch_all<A: AssetClass>(
    ctx: &CliContext,
    asset: &AssetSpec<A>,
    providers: &Providers,
    mode: PricesFrom,
) -> Vec<SourcePrices> {
    futures::future::join_all(asset.sources.iter().map(|source| async move {
        if source_inputs(source, mode) == "provider" {
            let (selected, stored) = futures::join!(providers.fetch(source), fetch(ctx, source));
            SourcePrices::Provider { selected, stored }
        } else {
            SourcePrices::Chain(fetch(ctx, source).await)
        }
    }))
    .await
}

fn stored_status(
    source: &SourceSpec,
    stored: &PriceResult,
    freshness: &FreshnessFilter,
    now: Nanoseconds,
) -> Status {
    match stored {
        Ok(Some(price)) if freshness.accepts(price, now) => Status::passed(format!(
            "stored chain price: {}",
            describe_price(source, price, now)
        )),
        Ok(Some(price)) => Status::warned(format!(
            "stored chain price outside freshness bounds (max age {}s, max drift {}s): {}",
            freshness.max_age_ns.map_or(0, |age| age.as_secs()),
            freshness
                .max_clock_drift_ns
                .map_or(0, |drift| drift.as_secs()),
            describe_price(source, price, now),
        )),
        Ok(None) => Status::warned(format!(
            "{} has no usable stored chain price",
            source.describe()
        )),
        Err(error) => Status::warned(format!(
            "{} stored chain price unreadable: {error:#}",
            source.describe()
        )),
    }
}

fn selected_status(
    source: &SourceSpec,
    fetched: &PriceResult,
    now: Nanoseconds,
    max_drift: Nanoseconds,
    drifted: bool,
    inputs: &str,
) -> Status {
    match fetched {
        Ok(Some(price)) if drifted => Status::failed(format!(
            "{inputs} input: {} is timestamped {}s in the future, beyond the {}s clock-drift \
             bound. The deployed oracle would reject it.",
            source.describe(),
            Nanoseconds::from_ns(price.publish_time_ns.as_ns().saturating_sub(now.as_ns())).as_secs(),
            max_drift.as_secs(),
        )),
        Ok(Some(price)) => Status::passed(format!("{inputs} input: {}", describe_price(source, price, now))),
        Ok(None) => Status::Skipped {
            reason: format!(
                "{inputs} input: {} carries no price yet, so it contributes nothing to this dry run",
                source.describe(),
            ),
        },
        Err(error) => Status::failed(format!("{inputs} input: {}: {error:#}", source.describe())),
    }
}

/// One side: fetch every source, report each, then aggregate.
fn leg<A: AssetClass>(
    side: &str,
    asset: &AssetSpec<A>,
    spec: &MarketSpec,
    fetched_sources: Vec<SourcePrices>,
    now: Nanoseconds,
    breakers: anyhow::Result<CircuitBreakerSet<CircuitBreaker>>,
    mode: PricesFrom,
) -> (Option<Price>, Vec<Check>) {
    let mut checks = Vec::new();
    let mut prices = Vec::with_capacity(asset.sources.len());

    // Drift is judged here, against wall-clock, and a drifted price is dropped
    // below rather than handed to `resolve`. That is what lets resolution use
    // the real clock: the deployed contract passes `env::block_timestamp`, so
    // resolving against anything else reports a freshness verdict the live
    // oracle would not give.
    let max_drift = asset.max_clock_drift.unwrap_or(DEFAULT_MAX_CLOCK_DRIFT);
    let drift_limit = Nanoseconds::from_ns(now.as_ns().saturating_add(max_drift.as_ns()));
    let freshness = FreshnessFilter::new(
        Some(asset.max_age.unwrap_or(spec.market.price_maximum_age)),
        Some(max_drift),
    );

    let mut transport_failed = false;
    for ((index, source), fetched) in asset.sources.iter().enumerate().zip(fetched_sources) {
        checks.push(Check::new(
            format!("oracle.stored.{side}.{index}"),
            stored_status(source, fetched.stored(), &freshness, now),
        ));
        let fetched = fetched.selected();
        let drifted = matches!(&fetched, Ok(Some(price)) if price.publish_time_ns > drift_limit);
        let inputs = source_inputs(source, mode);

        checks.push(Check::new(
            format!("oracle.price.{side}.{index}"),
            selected_status(source, fetched, now, max_drift, drifted, inputs),
        ));

        if fetched.is_err() {
            transport_failed = true;
        }
        // Order matters: `Proxy::resolve` zips these against its own source list
        // and rejects a length mismatch, so every source needs a slot — `None`
        // for one that did not answer, which is what `min_sources` weighs. A
        // drifted price is dropped too: the deployed oracle would not use it.
        prices.push(if drifted {
            None
        } else {
            fetched.as_ref().ok().and_then(|price| *price)
        });
    }

    // How many sources actually contributed. Distinguishes "nothing to judge"
    // from "judged and rejected", which decide Skipped vs Failed below.
    let live = prices.iter().flatten().count();
    let inputs = leg_inputs(asset, mode);

    // A breaker set that could not be read is not an empty one. Reported here
    // rather than resolved around, because resolving with an empty set removes
    // a rejection condition and can only turn a Failed into a Passed.
    let mut breakers = match breakers {
        Ok(breakers) => breakers,
        Err(error) => {
            checks.push(Check::new(
                format!("oracle.aggregate.{side}"),
                Status::failed(format!(
                    "{inputs} inputs: the deployed oracle's circuit breakers could not be read \
                     ({error:#}), so this aggregation cannot be judged. A tripped \
                     breaker would block every price."
                )),
            ));
            return (None, checks);
        }
    };

    // Cloned because `into_proxy` consumes, and the spec is still needed after.
    let proxy = asset.clone().into_proxy(spec.market.price_maximum_age);
    // `now`, not the newest fetched timestamp. Anchoring to the newest price
    // makes whichever source defines it age zero, so a set of feeds all stale by
    // the same amount every passes `max_age` here and is rejected on chain.
    let resolved = proxy.resolve(&mut breakers, prices, now);

    let (status, price) = match resolved {
        Ok(outcome) => match outcome.value {
            Ok(price) => (
                Status::passed(format!(
                    "{inputs} inputs: {} → {}",
                    aggregator_label(asset),
                    render(&price)
                )),
                Some(price),
            ),
            Err(reason) => (
                Status::failed(format!("{inputs} inputs: aggregation blocked: {reason:?}")),
                None,
            ),
        },
        // `Skipped` is only for "there was nothing to judge". If any source
        // produced a price and the aggregation still rejected the set, the
        // configuration is wrong and the deployed proxy would fail on the same
        // inputs — reporting that as skipped would exit zero and green-light the
        // deployment, since only failures are counted.
        Err(error) if live > 0 || transport_failed => (
            Status::failed(format!(
                "{inputs} inputs: {} could not aggregate ({error:?}) from {live} live source(s). \
                 The deployed proxy would fail on the same inputs — check \
                 `min_sources`, the freshness bounds, and the failed \
                 `oracle.price.{side}.*` above.",
                aggregator_label(asset)
            )),
            None,
        ),
        Err(error) => (
            Status::Skipped {
                reason: format!(
                    "{inputs} inputs: {} has no live sources to aggregate ({error:?}); no usable \
                     selected price was available.",
                    aggregator_label(asset)
                ),
            },
            None,
        ),
    };
    checks.push(Check::new(format!("oracle.aggregate.{side}"), status));

    (price, checks)
}

/// The collateral/borrow price ratio. Deliberately not decimals-adjusted:
/// decimals size a position, not a ratio of two USD prices, and applying them
/// reports 29.96 for a pair trading at 2.996.
fn pair(
    collateral: Option<Price>,
    borrow: Option<Price>,
    collateral_inputs: &str,
    borrow_inputs: &str,
) -> Check {
    let id = "oracle.aggregate.pair";
    let (Some(collateral), Some(borrow)) = (collateral, borrow) else {
        return Check::new(
            id,
            Status::Skipped {
                reason: format!("{collateral_inputs} collateral / {borrow_inputs} borrow inputs: both legs must aggregate before a ratio means anything"),
            },
        );
    };

    let borrow = scaled(&borrow);
    if borrow == 0.0 {
        return Check::new(
            id,
            Status::failed(format!("{collateral_inputs} collateral / {borrow_inputs} borrow inputs: the borrow leg aggregated to zero, so no ratio exists")),
        );
    }

    Check::new(
        id,
        Status::passed(format!(
            "{collateral_inputs} collateral / {borrow_inputs} borrow inputs: {} — sanity-check this against what the pair actually trades at",
            scaled(&collateral) / borrow
        )),
    )
}

fn render(price: &Price) -> String {
    format!("{}", scaled(price))
}

fn describe_price(source: &SourceSpec, price: &Price, now: Nanoseconds) -> String {
    let age_s =
        Nanoseconds::from_ns(now.as_ns().saturating_sub(price.publish_time_ns.as_ns())).as_secs();
    format!(
        "{} w{} {} age {age_s}s",
        source.describe(),
        source
            .weight()
            .map_or_else(|| "-".to_owned(), |w| w.to_string()),
        render(price)
    )
}

fn aggregator_label<A: AssetClass>(asset: &AssetSpec<A>) -> String {
    format!(
        "{:?} (min_sources {})",
        asset.aggregator.unwrap_or_default(),
        asset.min_sources
    )
}

/// One source's current price, projected exactly as the contract projects it.
///
/// `Ok(None)` means the adapter carries no price for this feed yet.
async fn fetch(ctx: &CliContext, source: &SourceSpec) -> anyhow::Result<Option<Price>> {
    let pyth_price: Option<pyth::Price> = match source {
        SourceSpec::Lazer {
            oracle, feed_id, ..
        } => {
            // The bulk read, because it is the one the deployed proxy makes.
            // An adapter serving only the singular form would pass here and
            // fail in production.
            ctx.client
                .read(templar_gateway_methods_spec::lazer::GetFeedsData {
                    oracle_id: oracle.clone(),
                    feed_ids: vec![*feed_id],
                })
                .await
                .with_context(|| format!("read lazer feed {feed_id} from {oracle}"))?
                .remove(feed_id)
                .flatten()
                // EMA, matching the adapter's own consumer path — spot would be
                // a different number than the market will see.
                .and_then(|feed| feed.to_ema_price())
        }
        SourceSpec::Pyth {
            oracle, price_id, ..
        } => ctx
            .client
            .read(templar_gateway_methods_spec::pyth::ListEmaPricesUnsafe {
                oracle_id: oracle.clone(),
                price_ids: vec![*price_id],
            })
            .await
            .with_context(|| format!("read pyth `{}` from {oracle}", hex::encode(price_id.0)))?
            .into_iter()
            .next()
            .and_then(|entry| entry.price),
        SourceSpec::RedStone {
            oracle, price_id, ..
        } => ctx
            .client
            .read(redstone::ReadPriceData {
                oracle_id: oracle.clone(),
                feed_ids: vec![price_id.clone().into()],
            })
            .await
            .with_context(|| format!("read redstone `{price_id}` from {oracle}"))?
            .first()
            .map(|entry| entry.data.clone())
            .as_ref()
            .and_then(redstone_types::FeedData::to_pyth_price),
        SourceSpec::Lst {
            oracle,
            price_id,
            contract,
            method,
            decimals,
            ..
        } => lst(ctx, oracle, *price_id, contract, method, *decimals).await?,
    };

    Ok(pyth_price.as_ref().and_then(pyth_price_try_to_kernel))
}

/// The underlying asset's price, scaled by the exchange rate a view on the
/// staking contract returns.
async fn lst(
    ctx: &CliContext,
    oracle: &near_account_id::AccountId,
    price_id: templar_common::oracle::pyth::PriceIdentifier,
    contract_id: &near_account_id::AccountId,
    method: &str,
    decimals: u32,
) -> anyhow::Result<Option<pyth::Price>> {
    let underlying = ctx
        .client
        .read(templar_gateway_methods_spec::pyth::ListEmaPricesUnsafe {
            oracle_id: oracle.clone(),
            price_ids: vec![price_id],
        })
        .await
        .with_context(|| format!("read pyth `{}` from {oracle}", hex::encode(price_id.0)))?
        .into_iter()
        .next()
        .and_then(|entry| entry.price);

    // Read before the underlying is required: a feed awaiting its first push is
    // expected, but a rate method that does not exist or answers with the wrong
    // shape is a source that can never produce a price, and the skip for the
    // former would hide the latter until after the market is live.
    let rate = ctx
        .client
        .read(contract::ViewFunction {
            contract_id: contract_id.clone(),
            method_name: method.to_owned().into(),
            args: ContractArgs::Json(serde_json::Value::Null),
        })
        .await
        .with_context(|| format!("read `{contract_id}.{method}`"))?;
    let rate: templar_common::Decimal = serde_json::from_value::<near_sdk::json_types::U128>(rate)
        .context("decode the LST exchange rate")?
        .0
        .into();

    let Some(underlying) = underlying else {
        return Ok(None);
    };

    // Scaled by the same `Action` the contract applies, not by a second
    // implementation of the same arithmetic here. The underlying answered, so a
    // `None` from it is the transform failing — `decimals >= 39` overflows its
    // scaling factor — not a feed awaiting its first push.
    Action::NormalizeNativeLstPrice { decimals }
        .apply(underlying, rate)
        .map(Some)
        .with_context(|| {
            format!(
                "`{contract_id}.{method}` returned {rate}, but scaling the \
                 underlying price by it at {decimals} decimals overflowed. This \
                 source can never produce a price."
            )
        })
}

#[cfg(test)]
mod tests;
