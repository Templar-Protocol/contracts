//! Ported from `contract/market/tests/snapshot.rs`. The original sleeps real
//! wall-clock to cross time-chunk boundaries; here we advance with `fast_forward`.
//! Generous advances are safe: `partial::check` skips snapshots that match the
//! previous expected state, so the extra (no-op) snapshots a large advance
//! produces are ignored — only the ordered sequence of *state changes* matters.
//! The `partial::check` / `states!` DSL is reused from `test-utils`.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::{
    dec, fee::Fee, interest_rate_strategy::InterestRateStrategy, time_chunk::TimeChunkConfiguration,
};
use templar_gateway_testing::{harness, SandboxHarness};
use test_utils::{partial::check, states};

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn snapshot_captures_borrow_and_collateral_state(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(500);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 2_000_000)
        .await?;

    harness
        .collateralize(&borrow_user, &market, 1_000_000)
        .await?;
    harness.borrow(&borrow_user, &market, 500_000).await?;

    harness.fast_forward(100).await?;
    // Snapshot updating occurs before the collateral deposit is recorded, so do
    // it twice to observe the preceding state in a finalized snapshot.
    harness.collateralize(&borrow_user, &market, 1).await?;
    harness.collateralize(&borrow_user, &market, 1).await?;

    let snapshots = harness.list_finalized_snapshots(&market).await?;
    check(
        states!(
            { active += 2_000_000 },
            { collateral += 1_000_000 },
            { borrowed += 500_000 },
            { collateral += 1 },
        ),
        snapshots,
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn multiple_snapshots_show_progression(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(1000);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let user = harness.create_user("user").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 3_000_000)
        .await?;
    harness.fast_forward(100).await?;

    harness.collateralize(&user, &market, 1_000_000).await?;
    harness.fast_forward(100).await?;
    harness.borrow(&user, &market, 400_000).await?;
    harness.fast_forward(100).await?;
    harness.borrow(&user, &market, 200_000).await?;
    harness.fast_forward(100).await?;
    harness
        .apply_interest(&user, &market, Some(user.0.clone()), None)
        .await?;

    let snapshots = harness.list_finalized_snapshots(&market).await?;
    check(
        states!(
            { active = 3_000_000 },
            { collateral += 1_000_000 },
            { borrowed += 400_000 },
            { borrowed += 200_000 },
        ),
        snapshots,
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn snapshot_reflects_repayment_changes(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_interest_rate_strategy = InterestRateStrategy::zero();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(500);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 2_000_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, 1_000_000)
        .await?;
    harness.borrow(&borrow_user, &market, 500_000).await?;

    harness.fast_forward(100).await?;
    harness.collateralize(&borrow_user, &market, 1).await?;
    let after_borrow = harness.list_finalized_snapshots(&market).await?.len();

    harness.repay(&borrow_user, &market, 250_000, None).await?;

    harness.fast_forward(100).await?;
    harness.collateralize(&borrow_user, &market, 1).await?;
    let snapshots = harness.list_finalized_snapshots(&market).await?;
    let after_repay = snapshots.len();

    assert!(after_repay > after_borrow);
    let borrowed_after_borrow = u128::from(snapshots[after_borrow - 1].borrow_asset_borrowed);
    let borrowed_after_repay = u128::from(snapshots[after_repay - 1].borrow_asset_borrowed);
    assert_eq!(
        borrowed_after_borrow,
        borrowed_after_repay * 2,
        "snapshots should reflect the halved borrowed amount",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn snapshot_handles_zero_operations(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.time_chunk_configuration = TimeChunkConfiguration::new(500);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    harness.fund_user(&supply_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1_000_000)
        .await?;
    let initial = harness.list_finalized_snapshots(&market).await?.len();

    // Let a chunk elapse with no activity, then trigger a snapshot.
    harness.fast_forward(100).await?;
    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1)
        .await?;
    let final_len = harness.list_finalized_snapshots(&market).await?.len();
    assert!(final_len > initial);

    harness
        .harvest_yield(&supply_user, &market, Some(supply_user.0.clone()))
        .await?;
    let snapshots = harness.list_finalized_snapshots(&market).await?;
    check(states!({ active = 1_000_000 }, { active += 1 }), snapshots);

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn snapshot_with_full_repayment(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(500);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 2_000_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, 1_000_000)
        .await?;
    harness.borrow(&borrow_user, &market, 500_000).await?;

    harness.fast_forward(100).await?;
    harness.collateralize(&borrow_user, &market, 1).await?;

    let total_liability = u128::from(
        harness
            .get_borrow_position(&market, &borrow_user.0)
            .await?
            .context("borrow position missing")?
            .get_total_borrow_asset_liability(),
    );
    harness
        .repay(&borrow_user, &market, total_liability, None)
        .await?;

    harness.fast_forward(100).await?;
    harness.collateralize(&borrow_user, &market, 1).await?;

    let snapshots = harness.list_finalized_snapshots(&market).await?;
    let last = snapshots.last().context("no snapshots")?;
    assert!(
        u128::from(last.borrow_asset_borrowed) <= 1000,
        "snapshot should show minimal borrowed after full repayment",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn snapshot_field_validation(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("2000"), dec!("3000")).unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(500);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1_500_000)
        .await?;
    harness.fast_forward(100).await?;
    harness.collateralize(&borrow_user, &market, 1).await?;

    harness
        .collateralize(&borrow_user, &market, 800_000)
        .await?;
    harness.fast_forward(100).await?;
    harness.collateralize(&borrow_user, &market, 1).await?;

    harness.borrow(&borrow_user, &market, 400_000).await?;
    harness.fast_forward(100).await?;
    harness.collateralize(&borrow_user, &market, 1).await?;

    harness.fast_forward(100).await?;
    harness.collateralize(&borrow_user, &market, 1).await?;
    harness.collateralize(&borrow_user, &market, 1).await?;

    let snapshots = harness.list_finalized_snapshots(&market).await?;
    check(
        states!(
            { active = 1_500_000 },
            { collateral += 1 },
            { collateral += 800_000 },
            { collateral += 1 },
            { borrowed += 400_000 },
            { collateral += 1 },
            { collateral += 1 },
        ),
        &snapshots,
    );

    // Timestamps are monotonic and the interest rate reflects utilization.
    let mut last_ts = snapshots[0].end_timestamp_ms.0;
    for (i, snapshot) in snapshots.iter().enumerate() {
        assert!(
            snapshot.end_timestamp_ms.0 >= last_ts,
            "timestamp decreased at {i}"
        );
        last_ts = snapshot.end_timestamp_ms.0;
    }
    assert!(
        !snapshots
            .last()
            .context("no snapshots")?
            .interest_rate
            .is_zero(),
        "interest rate should be positive with borrowing activity",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn many_users_same_snapshot(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(10_000);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_1 = harness.create_user("supply1").await?;
    let supply_2 = harness.create_user("supply2").await?;
    let users: Vec<_> = create_users(&harness, 5).await?;
    harness.fund_user(&supply_1, &market).await?;
    harness.fund_user(&supply_2, &market).await?;
    for user in &users {
        harness.fund_user(user, &market).await?;
    }

    // Supply both BEFORE either activates, then activate them together so a
    // single snapshot reflects all 3_500_000 active (not one per supplier).
    harness.supply(&supply_1, &market, 2_000_000).await?;
    harness.supply(&supply_2, &market, 1_500_000).await?;
    harness.fast_forward(1000).await?;
    harness
        .harvest_yield(&supply_1, &market, Some(supply_1.0.clone()))
        .await?;
    harness
        .harvest_yield(&supply_2, &market, Some(supply_2.0.clone()))
        .await?;

    // Run each group concurrently so all five land in the same time chunk
    // (sequential calls would straddle a chunk boundary and split the snapshot).
    harness.fast_forward(1000).await?;
    let collaterals = [400_000u128, 350_000, 300_000, 250_000, 200_000];
    let (a, b, c, d, e) = tokio::join!(
        harness.collateralize(&users[0], &market, collaterals[0]),
        harness.collateralize(&users[1], &market, collaterals[1]),
        harness.collateralize(&users[2], &market, collaterals[2]),
        harness.collateralize(&users[3], &market, collaterals[3]),
        harness.collateralize(&users[4], &market, collaterals[4]),
    );
    (a?, b?, c?, d?, e?);

    harness.fast_forward(1000).await?;
    let borrows = [150_000u128, 120_000, 100_000, 80_000, 60_000];
    let (a, b, c, d, e) = tokio::join!(
        harness.borrow(&users[0], &market, borrows[0]),
        harness.borrow(&users[1], &market, borrows[1]),
        harness.borrow(&users[2], &market, borrows[2]),
        harness.borrow(&users[3], &market, borrows[3]),
        harness.borrow(&users[4], &market, borrows[4]),
    );
    (a?, b?, c?, d?, e?);

    harness.fast_forward(1000).await?;

    harness
        .harvest_yield(&supply_1, &market, Some(supply_1.0.clone()))
        .await?;

    let snapshots = harness.list_finalized_snapshots(&market).await?;
    check(
        states!(
            { active += 2_000_000 + 1_500_000 },
            { collateral += 400_000 + 350_000 + 300_000 + 250_000 + 200_000 },
            { borrowed += 150_000 + 120_000 + 100_000 + 80_000 + 60_000 },
        ),
        snapshots,
    );

    Ok(())
}

async fn create_users(
    harness: &SandboxHarness,
    n: usize,
) -> Result<Vec<templar_gateway_types::ManagedAccountId>> {
    let mut users = Vec::with_capacity(n);
    for i in 0..n {
        users.push(harness.create_user(&format!("user{i}")).await?);
    }
    Ok(users)
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn incoming(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    harness.fund_user(&supply_user, &market).await?;

    harness.supply(&supply_user, &market, 2_000_000).await?;
    let activates_at = harness
        .get_supply_position(&market, &supply_user.0)
        .await?
        .context("supply position missing")?
        .get_deposit()
        .incoming[0]
        .activate_at_snapshot_index;

    while !harness
        .get_supply_position(&market, &supply_user.0)
        .await?
        .context("supply position missing")?
        .get_deposit()
        .incoming
        .is_empty()
    {
        harness
            .harvest_yield(&supply_user, &market, Some(supply_user.0.clone()))
            .await?;
    }
    // A few more snapshots after activation.
    for _ in 0..3 {
        harness
            .harvest_yield(&supply_user, &market, Some(supply_user.0.clone()))
            .await?;
    }

    let snapshots = harness.list_finalized_snapshots(&market).await?;
    let at = activates_at as usize;
    assert!(snapshots[at - 2].borrow_asset_deposited_active.is_zero());
    assert!(snapshots[at - 1].borrow_asset_deposited_active.is_zero());
    assert_eq!(
        u128::from(snapshots[at].borrow_asset_deposited_active),
        2_000_000
    );
    assert_eq!(
        u128::from(snapshots[at + 1].borrow_asset_deposited_active),
        2_000_000
    );

    Ok(())
}
