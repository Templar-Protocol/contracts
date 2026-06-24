//! Ported from `contract/market/tests/message_regression.rs`: a smoke test that
//! every deposit-message variant and market op still works end-to-end (Supply,
//! Collateralize, Repay, RepayAccount, withdraw collateral, apply interest, and
//! Liquidate). Each step asserts success via the harness `execute` path. The
//! `DepositMsg` wire format itself is covered by pure tests in `templar-common`.

use anyhow::Result;
use rstest::rstest;
use templar_common::interest_rate_strategy::InterestRateStrategy;
use templar_gateway_testing::{harness, SandboxHarness};

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn message_regression(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_interest_rate_strategy = InterestRateStrategy::zero();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;

    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    let third_party = harness.create_user("third").await?;
    for user in [&supply_user, &borrow_user, &third_party] {
        harness.fund_user(user, &market).await?;
    }

    // Supply + harvest.
    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000_000)
        .await?;

    // Collateralize + borrow.
    harness
        .collateralize(&borrow_user, &market, 2_000_000)
        .await?;
    harness.borrow(&borrow_user, &market, 1_000_000).await?;

    // Repay (self) and RepayAccount (third party repaying for the borrower).
    harness.repay(&borrow_user, &market, 250_000, None).await?;
    harness
        .repay(&third_party, &market, 250_000, Some(borrow_user.0.clone()))
        .await?;

    // Withdraw collateral + apply interest.
    harness
        .withdraw_collateral(&borrow_user, &market, 1_000_000)
        .await?;
    harness
        .apply_interest(
            &third_party,
            &market,
            Some(borrow_user.0.clone()),
            Some(100),
        )
        .await?;

    // Drop the collateral's value so the position is liquidatable, then liquidate.
    harness.set_asset_prices(&market, 2.0, 1.0).await?;
    harness
        .liquidate(
            &third_party,
            &market,
            &borrow_user.0,
            500_000,
            Some(1_000_000),
        )
        .await?;

    Ok(())
}
