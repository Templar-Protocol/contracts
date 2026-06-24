//! Ported from `contract/market/tests/interest_rate.rs`, split per the plan:
//! the *exact* interest arithmetic is covered by the deterministic, node-free
//! `templar-common` unit test `calculate_interest_two_snapshots_exact`. This
//! integration test covers the behavior that genuinely needs a node: interest
//! accrues as (fast-forwarded) time passes, and applying it more frequently
//! cannot reduce the total a borrower owes.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::{dec, fee::Fee, interest_rate_strategy::InterestRateStrategy};
use templar_gateway_testing::{harness, DeployedMarket, SandboxHarness};

async fn realized_interest(
    harness: &SandboxHarness,
    market: &DeployedMarket,
    user: &templar_gateway_types::ManagedAccountId,
) -> Result<u128> {
    Ok(u128::from(
        harness
            .get_borrow_position(market, &user.0)
            .await?
            .context("borrow position missing")?
            .interest
            .get_total(),
    ))
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn interest_accrues_and_frequency_does_not_reduce_it(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    // `eager` applies interest repeatedly as time passes; `lazy` only at the end.
    let eager = harness.create_user("eager").await?;
    let lazy = harness.create_user("lazy").await?;
    for user in [&supply_user, &eager, &lazy] {
        harness.fund_user(user, &market).await?;
    }

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 100_000_000)
        .await?;
    harness.collateralize(&eager, &market, 10_000_000).await?;
    harness.collateralize(&lazy, &market, 10_000_000).await?;
    harness.borrow(&eager, &market, 1_000_000).await?;
    harness.borrow(&lazy, &market, 1_000_000).await?;

    // Advance time in chunks, applying interest to `eager` each chunk.
    for _ in 0..3 {
        harness.fast_forward(100).await?;
        harness
            .apply_interest(&eager, &market, Some(eager.0.clone()), None)
            .await?;
    }
    // Realize both fully so the totals are comparable.
    harness
        .apply_interest(&eager, &market, Some(eager.0.clone()), None)
        .await?;
    harness
        .apply_interest(&lazy, &market, Some(lazy.0.clone()), None)
        .await?;

    let eager_interest = realized_interest(&harness, &market, &eager).await?;
    let lazy_interest = realized_interest(&harness, &market, &lazy).await?;

    assert!(eager_interest > 0, "interest should accrue over time");
    assert!(lazy_interest > 0, "interest should accrue over time");
    assert!(
        eager_interest >= lazy_interest,
        "applying interest more often must not reduce it (eager {eager_interest} < lazy {lazy_interest})",
    );
    // ...and it should barely differ — frequent application only rounds up.
    assert!(
        eager_interest - lazy_interest <= lazy_interest / 100 + 100,
        "frequent application changed interest too much (eager {eager_interest} vs lazy {lazy_interest})",
    );

    Ok(())
}
