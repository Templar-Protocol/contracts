//! Offline aggregation dry-run: fetch each upstream feed, then run the proxy's
//! own aggregation locally.
//!
//! Nothing needs to be deployed. The oracle this market will use does not exist
//! yet, so the adapters are queried directly and
//! [`Proxy::resolve`](templar_proxy_oracle_kernel::proxy::Proxy::resolve) — pure
//! `no_std`, the same code the contract runs — is applied to the results against
//! an empty circuit-breaker set. That makes the number a market would actually
//! consume visible *before* the first transaction.
//!
//! Absence of a price is not a failure. An adapter carries a feed once someone
//! pushes one, so a market deployed for the first time may name a feed with no
//! data yet; that is reported, not rejected (see ENG-541).

use anyhow::Context as _;
use templar_common::asset::AssetClass;
use templar_common::oracle::{lazer, pyth, redstone as redstone_types};
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

/// `oracle.aggregate.{collateral,borrow,pair}`.
///
/// Both legs are fetched first, then judged against a single clock and a single
/// anchor — mirroring the deployed contract, whose callback captures one block
/// timestamp *after* every oracle result has arrived and resolves both legs
/// against it. Per-leg clocks let a fresh collateral and a week-old borrow each
/// pass on their own terms and produce a ratio the contract could never accept.
pub(super) async fn checks(
    ctx: &CliContext,
    spec: &MarketSpec,
    deployed_oracle: Option<&near_account_id::AccountId>,
) -> (Vec<Check>, Option<Price>, Option<Price>) {
    // Nothing to dry-run for a direct market: this reproduces a *proxy's*
    // aggregation, and an oracle we did not configure has none of ours to
    // reproduce. Reported as not run rather than silently passing.
    if spec.oracle.is_direct() {
        return (
            vec![Check::new(
                "oracle.aggregate",
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
    let now = wall_clock();

    // One anchor for both legs, capped at wall-clock.
    //
    // Capped, because a source timestamped in the future but *within*
    // `max_clock_drift` is legitimate to the contract, yet as an anchor it would
    // age every honest peer by that difference. Shared, because the contract
    // resolves both legs against the same instant.
    let newest = collateral
        .iter()
        .chain(borrow.iter())
        .filter_map(|fetched| fetched.as_ref().ok()?.as_ref())
        .map(|price| price.publish_time_ns)
        .max()
        .unwrap_or(now);
    let anchor = newest.min(now);

    // Against the oracle's own breakers when one is deployed. An empty set is
    // right for `market plan` — the oracle does not exist yet — and wrong for
    // `market verify`: a tripped breaker means the live oracle prices nothing,
    // and resolving without it would report the aggregation healthy for a
    // market that is blocked.
    let collateral_breakers = breakers(ctx, deployed_oracle, COLLATERAL_PRICE_ID).await;
    let borrow_breakers = breakers(ctx, deployed_oracle, BORROW_PRICE_ID).await;

    let (collateral_price, mut checks) = leg(
        "collateral",
        &spec.collateral,
        spec,
        collateral,
        now,
        anchor,
        collateral_breakers,
    );
    let (borrow_price, borrow_checks) = leg(
        "borrow",
        &spec.borrow,
        spec,
        borrow,
        now,
        anchor,
        borrow_breakers,
    );
    checks.extend(borrow_checks);
    checks.push(pair(collateral_price, borrow_price));
    (checks, collateral_price, borrow_price)
}

/// The oracle's configured breakers for a feed, or an empty set.
///
/// Empty when no oracle is deployed (planning) or when the set cannot be read —
/// the latter is reported by `oracle.aggregate.*` failing to resolve rather than
/// silently passing, since a proxy with no set simply has nothing to trip.
async fn breakers(
    ctx: &CliContext,
    oracle_id: Option<&near_account_id::AccountId>,
    id: templar_common::oracle::pyth::PriceIdentifier,
) -> CircuitBreakerSet<CircuitBreaker> {
    let Some(oracle_id) = oracle_id else {
        return CircuitBreakerSet::empty();
    };
    ctx.client
        .read(
            templar_gateway_methods_spec::proxy_oracle::GetProxyCircuitBreakerSet {
                oracle_id: oracle_id.clone(),
                id,
            },
        )
        .await
        .ok()
        .and_then(|result| result.circuit_breaker_set)
        .unwrap_or_else(CircuitBreakerSet::empty)
}

/// Wall-clock, for freshness. The kernel takes `now` explicitly rather than
/// reading a clock, which is what makes it testable; this is the one place a
/// clock is read.
fn wall_clock() -> Nanoseconds {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Nanoseconds::from_ns(u64::try_from(since_epoch.as_nanos()).unwrap_or(u64::MAX))
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
    anchor: Nanoseconds,
    mut breakers: CircuitBreakerSet<CircuitBreaker>,
) -> (Option<Price>, Vec<Check>) {
    let mut checks = Vec::new();
    let mut prices = Vec::with_capacity(asset.sources.len());

    // Clock drift is checked against wall-clock, before anything is anchored.
    //
    // The anchor below is the newest fetched price, so whichever source defines
    // it is always evaluated at age zero. That makes the filter's own
    // `max_clock_drift` bound unreachable here — a source with a bogus *future*
    // timestamp would become the anchor, pass trivially, and push every honest
    // peer past `max_age`, so the dry run would report a price built from the
    // one bad source while production rejected exactly that source. Drift is
    // only meaningful against a real clock, so it is judged here instead.
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

    // Cloned because `into_proxy` consumes, and the spec is still needed after.
    let proxy = asset.clone().into_proxy(spec.market.price_maximum_age);
    let resolved = proxy.resolve(&mut breakers, prices, anchor);

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

/// The collateral/borrow price ratio — the number a human recognizes.
///
/// Deliberately *not* decimals-adjusted. Decimals convert a raw token amount
/// into a value when the market sizes a position; they play no part in the ratio
/// of two USD prices. Applying them here would report 29.96 for a pair that
/// trades at 2.996 — plausible enough to be believed, which is precisely the
/// class of error this check exists to catch.
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
            let result = ctx
                .client
                .read(contract::ViewFunction {
                    contract_id: oracle.clone(),
                    method_name: "get_feed_data".to_owned().into(),
                    args: ContractArgs::Json(serde_json::json!({ "feed_id": feed_id })),
                })
                .await
                .with_context(|| format!("read lazer feed {feed_id} from {oracle}"))?;

            serde_json::from_value::<Option<lazer::FeedData>>(result.value)
                .context("decode lazer feed data")?
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
        } => {
            let underlying = ctx
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
                .and_then(|entry| entry.price);

            // Scaled by the same `Action` the contract applies, not by a second
            // implementation of the same arithmetic here.
            match underlying {
                None => None,
                Some(underlying) => {
                    let rate = ctx
                        .client
                        .read(contract::ViewFunction {
                            contract_id: contract.clone(),
                            method_name: method.clone().into(),
                            args: ContractArgs::Json(serde_json::Value::Null),
                        })
                        .await
                        .with_context(|| format!("read `{contract}.{method}`"))?;
                    let rate: templar_common::Decimal =
                        serde_json::from_value::<near_sdk::json_types::U128>(rate.value)
                            .context("decode the LST exchange rate")?
                            .0
                            .into();

                    Action::NormalizeNativeLstPrice {
                        decimals: *decimals,
                    }
                    .apply(underlying, rate)
                }
            }
        }
    };

    Ok(pyth_price.as_ref().and_then(pyth_price_try_to_kernel))
}
