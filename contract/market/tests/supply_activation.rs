//! Ported from `contract/market/tests/supply_activation.rs`.
//!
//! The original relied on 1ms snapshots ticking between rapid near-workspaces
//! calls. Under the gateway's slower multi-step ops that timing is unstable, so
//! we instead use long snapshots (which don't tick on their own during the
//! test) and advance time deterministically with `fast_forward`. Same condition:
//! a fresh deposit activates in the *next* snapshot, not the current one.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::{
    fee::Fee, interest_rate_strategy::InterestRateStrategy, time_chunk::TimeChunkConfiguration,
};
use templar_gateway_testing::{harness, SandboxHarness};

// Long enough that a snapshot doesn't tick between reading the snapshot count
// and supplying (a few seconds of harness ops), short enough that `fast_forward`
// crosses it cheaply.
const SNAPSHOT_MS: u64 = 20_000;

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn activates_in_next_snapshot(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy = InterestRateStrategy::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(SNAPSHOT_MS);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .collateralize(&borrow_user, &market, 2_000_000)
        .await?;

    let snapshots_before = harness.get_finalized_snapshots_len(&market).await?;
    harness.supply(&supply_user, &market, 1_000_000).await?;

    // The fresh deposit is scheduled for the next snapshot, not active now.
    let position = harness
        .get_supply_position(&market, &supply_user.0)
        .await?
        .context("supply position missing")?;
    let activate_at = position.get_deposit().incoming[0].activate_at_snapshot_index;
    assert_eq!(
        activate_at,
        snapshots_before + 1,
        "funds should activate in the next snapshot",
    );

    // Advance well past the snapshot boundary so the deposit is unambiguously
    // active by the time we borrow (the borrow finalizes the elapsed snapshots).
    harness.fast_forward(1000).await?;

    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &borrow_user.0)
        .await?;
    harness.borrow(&borrow_user, &market, 1_000).await?;
    let balance_after = harness
        .ft_balance_of(&market.borrow_ft_id, &borrow_user.0)
        .await?;
    assert_eq!(
        balance_before + 1_000,
        balance_after,
        "supplied funds should be borrowable after activation",
    );

    Ok(())
}
