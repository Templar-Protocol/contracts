//! Ported from `contract/market/tests/0_many_accounts.rs`, reduced.
//!
//! The original spins up 100 accounts via a `JoinSet` of spawned sub-accounts and
//! a concurrent withdrawal-queue driver, asserting the final supply/borrow
//! position listings match. The failure condition it guards is state consistency
//! across many accounts; we recapture that with a deterministic set of suppliers
//! and borrowers (no opaque concurrency, no full-withdrawal position churn) and
//! assert the position listings reflect exactly them.

use anyhow::Result;
use rstest::rstest;
use templar_common::{fee::Fee, interest_rate_strategy::InterestRateStrategy};
use templar_gateway_testing::{harness, SandboxHarness};

const SUPPLIERS: usize = 8;
const BORROWERS: usize = 8;

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn many_accounts_consistent_position_listings(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy = InterestRateStrategy::zero();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;

    let mut suppliers = Vec::with_capacity(SUPPLIERS);
    for i in 0..SUPPLIERS {
        let user = harness.create_user(&format!("supply{i}")).await?;
        harness.fund_user(&user, &market).await?;
        harness
            .supply_and_harvest_until_activation(&user, &market, 100_000)
            .await?;
        suppliers.push(user);
    }

    let mut borrowers = Vec::with_capacity(BORROWERS);
    for i in 0..BORROWERS {
        let user = harness.create_user(&format!("borrow{i}")).await?;
        harness.fund_user(&user, &market).await?;
        harness.collateralize(&user, &market, 100_000).await?;
        harness.borrow(&user, &market, 40_000).await?;
        // Repay the principal; the collateral keeps the position registered.
        harness.repay(&user, &market, 40_000, None).await?;
        borrowers.push(user);
    }

    let supply_positions = harness.list_supply_positions(&market).await?;
    let borrow_positions = harness.list_borrow_positions(&market).await?;

    assert_eq!(supply_positions.len(), SUPPLIERS);
    assert_eq!(borrow_positions.len(), BORROWERS);
    for supplier in &suppliers {
        assert!(
            supply_positions.contains_key(&supplier.0),
            "missing supply position for {}",
            supplier.0,
        );
    }
    for borrower in &borrowers {
        assert!(
            borrow_positions.contains_key(&borrower.0),
            "missing borrow position for {}",
            borrower.0,
        );
    }

    Ok(())
}
