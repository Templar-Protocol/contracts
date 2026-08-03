//! Offline aggregation dry-run: query each adapter directly, then apply
//! [`Proxy::resolve`](templar_proxy_oracle_kernel::proxy::Proxy::resolve) — the
//! same `no_std` code the contract runs — so the number a market would consume
//! is visible before the first transaction.
//!
//! Absence of a price is reported, not rejected: an adapter carries a feed only
//! once someone pushes one.

use anyhow::Context as _;
use templar_common::asset::AssetClass;
use templar_common::oracle::{pyth, redstone as redstone_types};
use templar_common::Nanoseconds;
use templar_gateway_methods_spec::{contract, redstone};
use templar_gateway_types::common::ContractArgs;
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{CircuitBreaker, CircuitBreakerSet};
use templar_proxy_oracle_kernel::Price;
use templar_proxy_oracle_near_common::convert::pyth_price_try_to_kernel;
use templar_proxy_oracle_near_common::price_transformer::Action;

use super::scaled;
use crate::context::CliContext;
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
) -> (Vec<Check>, Option<Price>, Option<Price>) {
    // Nothing to dry-run for a direct market: this reproduces a *proxy's*
    // aggregation, and an oracle we did not configure has none of ours to
    // reproduce. Reported as not run rather than silently passing. Its prices
    // still reach the reference cross-check — `oracle.serves_pair` reads them.
    if spec.oracle.is_direct() {
        return (
            vec![Check::new(
                "oracle.aggregate.all",
                Status::Skipped {
                    reason: "this market reads an existing oracle; there is no \
                             proxy aggregation to reproduce"
                        .to_owned(),
                },
            )],
            None,
            None,
        );
    }

    let collateral = fetch_all(ctx, &spec.collateral).await;
    let borrow = fetch_all(ctx, &spec.borrow).await;

    // Sampled after the fetches, not before. Sequential RPCs can outlast the
    // drift allowance, and a feed updated mid-sweep would then read as
    // future-drifted against a clock taken before it was even requested.
    let now = crate::spec::wall_clock();

    // Against the oracle's own breakers when one is deployed. An empty set is
    // right for `market plan` — the oracle does not exist yet — and wrong for
    // `market verify`: a tripped breaker means the live oracle prices nothing,
    // and resolving without it would report the aggregation healthy for a
    // market that is blocked.
    let collateral_breakers = breakers(ctx, deployed_oracle, COLLATERAL_PRICE_ID).await;
    let borrow_breakers = breakers(ctx, deployed_oracle, BORROW_PRICE_ID).await;

    // A deployed oracle means `market verify`, not `market plan`: feeds that
    // resolve to nothing are a market that cannot price, not one awaiting its
    // first push.
    let live_market = deployed_oracle.is_some();

    let (collateral_price, mut checks) = leg(
        "collateral",
        &spec.collateral,
        spec,
        collateral,
        now,
        collateral_breakers,
        live_market,
    );
    let (borrow_price, borrow_checks) = leg(
        "borrow",
        &spec.borrow,
        spec,
        borrow,
        now,
        borrow_breakers,
        live_market,
    );
    checks.extend(borrow_checks);
    checks.push(pair(collateral_price, borrow_price));
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
    Ok(result
        .circuit_breaker_set
        .unwrap_or_else(CircuitBreakerSet::empty))
}

/// Every source's current price, in spec order.
async fn fetch_all<A: AssetClass>(
    ctx: &CliContext,
    asset: &AssetSpec<A>,
) -> Vec<anyhow::Result<Option<Price>>> {
    let mut fetched = Vec::with_capacity(asset.sources.len());
    for source in &asset.sources {
        fetched.push(fetch(ctx, source).await);
    }
    fetched
}

/// One side: fetch every source, report each, then aggregate.
fn leg<A: AssetClass>(
    side: &str,
    asset: &AssetSpec<A>,
    spec: &MarketSpec,
    fetched_sources: Vec<anyhow::Result<Option<Price>>>,
    now: Nanoseconds,
    breakers: anyhow::Result<CircuitBreakerSet<CircuitBreaker>>,
    live_market: bool,
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

    let mut transport_failed = false;
    for ((index, source), fetched) in asset.sources.iter().enumerate().zip(fetched_sources) {
        let fetched = &fetched;
        let drifted = matches!(&fetched, Ok(Some(price)) if price.publish_time_ns > drift_limit);

        checks.push(Check::new(
            format!("oracle.price.{side}.{index}"),
            match &fetched {
                Ok(Some(price)) if drifted => Status::failed(format!(
                    "{} is timestamped {}s in the future, beyond the {}s clock-drift \
                     bound. The deployed oracle would reject it.",
                    source.describe(),
                    Nanoseconds::from_ns(price.publish_time_ns.as_ns().saturating_sub(now.as_ns()))
                        .as_secs(),
                    max_drift.as_secs(),
                )),
                Ok(Some(price)) => Status::passed(describe_price(source, price, now)),
                Ok(None) => Status::Skipped {
                    reason: format!(
                        "{} carries no price yet, so it contributes nothing to this \
                         dry run",
                        source.describe()
                    ),
                },
                Err(error) => Status::failed(format!("{}: {error:#}", source.describe())),
            },
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

    // A breaker set that could not be read is not an empty one. Reported here
    // rather than resolved around, because resolving with an empty set removes
    // a rejection condition and can only turn a Failed into a Passed.
    let mut breakers = match breakers {
        Ok(breakers) => breakers,
        Err(error) => {
            checks.push(Check::new(
                format!("oracle.aggregate.{side}"),
                Status::failed(format!(
                    "the deployed oracle's circuit breakers could not be read \
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
                Status::passed(format!("{} → {}", aggregator_label(asset), render(&price))),
                Some(price),
            ),
            Err(reason) => (
                Status::failed(format!("aggregation blocked: {reason:?}")),
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
                "{} could not aggregate ({error:?}) from {live} live source(s). \
                 The deployed proxy would fail on the same inputs — check \
                 `min_sources`, the freshness bounds, and the failed \
                 `oracle.price.{side}.*` above.",
                aggregator_label(asset)
            )),
            None,
        ),
        // Nothing to judge is expected before the first push, but this market is
        // already live: its oracle cannot price the {side} asset right now.
        Err(error) if live_market => (
            Status::failed(format!(
                "{} has no live sources ({error:?}), so the deployed oracle cannot \
                 price the {side} asset. Every borrow, repay and liquidation on \
                 this market is blocked until a source publishes.",
                aggregator_label(asset)
            )),
            None,
        ),
        Err(error) => (
            Status::Skipped {
                reason: format!(
                    "{} has no live sources to aggregate ({error:?}). For feeds \
                     awaiting their first push this is expected; re-run once they \
                     carry a price, before deploying.",
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
fn pair(collateral: Option<Price>, borrow: Option<Price>) -> Check {
    let id = "oracle.aggregate.pair";
    let (Some(collateral), Some(borrow)) = (collateral, borrow) else {
        return Check::new(
            id,
            Status::Skipped {
                reason: "both legs must aggregate before a ratio means anything".to_owned(),
            },
        );
    };

    let borrow = scaled(&borrow);
    if borrow == 0.0 {
        return Check::new(
            id,
            Status::failed("the borrow leg aggregated to zero, so no ratio exists".to_owned()),
        );
    }

    Check::new(
        id,
        Status::passed(format!(
            "{} — sanity-check this against what the pair actually trades at",
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
                .feeds
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
            .prices
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
            .entries
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
        .prices
        .into_iter()
        .next()
        .and_then(|entry| entry.price);

    let Some(underlying) = underlying else {
        return Ok(None);
    };

    let rate = ctx
        .client
        .read(contract::ViewFunction {
            contract_id: contract_id.clone(),
            method_name: method.to_owned().into(),
            args: ContractArgs::Json(serde_json::Value::Null),
        })
        .await
        .with_context(|| format!("read `{contract_id}.{method}`"))?;
    let rate: templar_common::Decimal =
        serde_json::from_value::<near_sdk::json_types::U128>(rate.value)
            .context("decode the LST exchange rate")?
            .0
            .into();

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
