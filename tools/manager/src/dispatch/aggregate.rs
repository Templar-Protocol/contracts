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

use crate::context::CliContext;
use crate::spec::{
    check::{Check, Status},
    oracle::{AssetSpec, SourceSpec},
    MarketSpec,
};

/// `oracle.aggregate.{collateral,borrow,pair}`.
pub(super) async fn checks(ctx: &CliContext, spec: &MarketSpec, now: Nanoseconds) -> Vec<Check> {
    let (collateral, mut checks) = leg(ctx, "collateral", &spec.collateral, spec, now).await;
    let (borrow, borrow_checks) = leg(ctx, "borrow", &spec.borrow, spec, now).await;
    checks.extend(borrow_checks);
    checks.push(pair(collateral, borrow));
    checks
}

/// One side: fetch every source, report each, then aggregate.
async fn leg<A: AssetClass>(
    ctx: &CliContext,
    side: &str,
    asset: &AssetSpec<A>,
    spec: &MarketSpec,
    now: Nanoseconds,
) -> (Option<Price>, Vec<Check>) {
    let mut checks = Vec::new();
    let mut prices = Vec::with_capacity(asset.sources.len());

    for (index, source) in asset.sources.iter().enumerate() {
        let fetched = fetch(ctx, source).await;
        checks.push(Check::new(
            format!("oracle.price.{side}.{index}"),
            match &fetched {
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
        // Order matters: `Proxy::resolve` zips these against its own source list
        // and rejects a length mismatch, so every source needs a slot — `None`
        // for one that did not answer, which is what `min_sources` weighs.
        prices.push(fetched.ok().flatten());
    }

    // Freshness is measured between the sources, not against wall-clock.
    //
    // An adapter holds whatever price was last pushed to it; the relayer pushes
    // a fresh one when an operation needs it. So at rest a perfectly healthy
    // feed is routinely hours old, and filtering on wall-clock would report
    // every market as unaggregatable — a check that always fails tells you
    // nothing. Anchoring on the newest price still catches the failure that
    // matters here: one source lagging far behind its peers, which is what
    // `max_age` guards against once the oracle is live. Absolute ages are
    // reported per source above, so real staleness stays visible.
    let anchor = prices
        .iter()
        .flatten()
        .map(|price| price.publish_time_ns)
        .max()
        .unwrap_or(now);

    // Cloned because `into_proxy` consumes, and the spec is still needed after.
    let proxy = asset.clone().into_proxy(spec.market.price_maximum_age);
    // No breakers: the oracle does not exist yet, so there is no configured set
    // and nothing to trip. This measures the aggregation, not the guard rails.
    let mut breakers = CircuitBreakerSet::<CircuitBreaker>::empty();
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
        // Too few live sources is the expected shape for a market whose feeds
        // have not been pushed yet, so it reads as "could not run", not "wrong".
        Err(error) => (
            Status::Skipped {
                reason: format!(
                    "{} could not aggregate: {error:?}. With feeds that carry no \
                     price yet this is expected; once they do, re-run before \
                     deploying.",
                    aggregator_label(asset)
                ),
            },
            None,
        ),
    };
    checks.push(Check::new(format!("oracle.aggregate.{side}"), status));

    (price, checks)
}

/// The collateral/borrow price ratio — the number a human recognises.
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

/// A price as a plain number, for reporting only.
fn scaled(price: &Price) -> f64 {
    #[allow(
        clippy::cast_precision_loss,
        reason = "this value is displayed, never used for a decision"
    )]
    let value = price.price as f64;
    value * 10f64.powi(price.expo)
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
        source.weight(),
        render(price)
    )
}

fn aggregator_label<A: AssetClass>(asset: &AssetSpec<A>) -> String {
    format!("{:?} (min_sources {})", asset.aggregator, asset.min_sources)
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
    };

    Ok(pyth_price.as_ref().and_then(pyth_price_try_to_kernel))
}
