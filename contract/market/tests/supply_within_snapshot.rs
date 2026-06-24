//! Ported from `contract/market/tests/supply_within_snapshot.rs`.
//!
//! `funds_activation` is ported here. The original `partial_snapshot_no_earnings`
//! asserts *exact* per-snapshot yield amounts computed from snapshot timestamps —
//! the same exact-interest arithmetic covered deterministically (and node-free)
//! by the `templar-common` interest/yield unit tests — so it is not reproduced
//! as a flaky timestamp-bounded integration test.
//!
//! "Activates in one snapshot" is asserted via the deposit's
//! `activate_at_snapshot_index`, which the contract fixes (to the next snapshot)
//! at supply time — a deterministic fact, unlike how many snapshots happen to
//! finalize while we advance time.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::{
    dec, fee::Fee, interest_rate_strategy::InterestRateStrategy, market::YieldWeights,
    time_chunk::TimeChunkConfiguration,
};
use templar_gateway_testing::{harness, DeployedMarket, SandboxHarness};
use templar_gateway_types::ManagedAccountId;

async fn deposit_state(
    harness: &SandboxHarness,
    market: &DeployedMarket,
    user: &ManagedAccountId,
) -> Result<templar_common::supply::Deposit> {
    Ok(harness
        .get_supply_position(market, &user.0)
        .await?
        .context("supply position missing")?
        .get_deposit()
        .clone())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn funds_activate_in_the_next_snapshot(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("10000"), dec!("10000")).unwrap();
            c.time_chunk_configuration = TimeChunkConfiguration::new(8 * 1000);
            c.yield_weights = YieldWeights::new_with_supply_weight(1);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    harness.fund_user(&supply_user, &market).await?;

    for round in 1..=2u128 {
        harness.supply(&supply_user, &market, 1_000_000).await?;

        let finalized_at_supply = harness.list_finalized_snapshots(&market).await?.len() as u64;
        let activate_at = u64::from(
            deposit_state(&harness, &market, &supply_user)
                .await?
                .incoming[0]
                .activate_at_snapshot_index,
        );
        // The deposit defers to exactly the next snapshot (one past the last
        // finalized one).
        assert_eq!(
            activate_at,
            finalized_at_supply + 1,
            "round {round}: deposit should activate one snapshot after supply",
        );

        // Advance until the deposit activates.
        while !deposit_state(&harness, &market, &supply_user)
            .await?
            .incoming
            .is_empty()
        {
            harness.fast_forward(500).await?;
            harness
                .harvest_yield(&supply_user, &market, Some(supply_user.0.clone()))
                .await?;
        }

        assert_eq!(
            u128::from(deposit_state(&harness, &market, &supply_user).await?.active),
            round * 1_000_000,
            "round {round}: all supplied funds should be active",
        );
    }

    Ok(())
}
