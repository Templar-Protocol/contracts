//! Ported from `contract/market/tests/borrow_limits.rs`. The original
//! `borrow_above_maximum` borrowed concurrently via a `JoinSet`; here it borrows
//! sequentially and asserts at least one borrow is rejected — borrows are direct
//! market calls that fail outright (unlike `ft_transfer_call` deposits, which
//! refund), so the rejection surfaces as a failed operation.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::{fee::Fee, interest_rate_strategy::InterestRateStrategy, Decimal};
use templar_gateway_testing::{harness, SandboxHarness};
use templar_gateway_types::OperationStatus;

const OUT_OF_RANGE: &str = "New borrow position is outside of allowable range";

#[rstest]
#[case(0, &[1], u128::MAX)]
#[case(1, &[1], u128::MAX)]
#[case(10, &[10], 10)]
#[case(0, &[20, 20, 20, 20, 20], 100)]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn borrow_within_bounds(
    #[future(awt)] harness: SandboxHarness,
    #[case] minimum: u128,
    #[case] amounts: &[u128],
    #[case] maximum: u128,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_range = (minimum, Some(maximum)).try_into().unwrap();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1000)
        .await?;
    harness.collateralize(&borrow_user, &market, 2000).await?;

    for &amount in amounts {
        harness.borrow(&borrow_user, &market, amount).await?;
    }

    assert_eq!(
        u128::from(
            harness
                .get_borrow_position(&market, &borrow_user.0)
                .await?
                .context("borrow position missing")?
                .get_borrow_asset_principal()
        ),
        amounts.iter().sum::<u128>(),
    );

    Ok(())
}

#[rstest]
#[case(2, 1, 2)]
#[case(100, 99, 1000)]
#[case(u128::MAX, 1, u128::MAX)]
#[case(1000, 738, u128::MAX)]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn borrow_below_minimum(
    #[future(awt)] harness: SandboxHarness,
    #[case] minimum: u128,
    #[case] amount: u128,
    #[case] maximum: u128,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_range = (minimum, Some(maximum)).try_into().unwrap();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1000)
        .await?;
    harness.collateralize(&borrow_user, &market, 2000).await?;

    let result = harness.try_borrow(&borrow_user, &market, amount).await?;
    assert_eq!(result.operation.status, OperationStatus::Failed);
    assert!(
        result
            .operation
            .failure_message()
            .unwrap_or_default()
            .contains(OUT_OF_RANGE),
        "unexpected failure reason: {:?}",
        result.operation.failure_message(),
    );

    Ok(())
}

#[rstest]
#[case(0, &[2], 1)]
#[case(0, &[1, 1], 1)]
#[case(0, &[1001], 1000)]
#[case(0, &[1000, 1], 1000)]
#[case(1000, &[1001], 1000)]
#[case(500, &[500, 501], 1000)]
#[case(100, &[1001], 500)]
#[case(100, &[100, 100, 100, 100, 100, 100, 100], 500)]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn borrow_above_maximum(
    #[future(awt)] harness: SandboxHarness,
    #[case] minimum: u128,
    #[case] amounts: &[u128],
    #[case] maximum: u128,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_range = (minimum, Some(maximum)).try_into().unwrap();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;
    harness.collateralize(&borrow_user, &market, 2000).await?;

    let mut any_rejected = false;
    for &amount in amounts {
        let result = harness.try_borrow(&borrow_user, &market, amount).await?;
        if result.operation.status == OperationStatus::Failed {
            any_rejected = true;
            assert!(
                result
                    .operation
                    .failure_message()
                    .unwrap_or_default()
                    .contains(OUT_OF_RANGE),
                "unexpected failure reason: {:?}",
                result.operation.failure_message(),
            );
        }
    }
    assert!(
        any_rejected,
        "expected at least one borrow to be rejected for exceeding the maximum",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn withdraw_below_minimum(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_range = (10, None).try_into().unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1000)
        .await?;
    harness.collateralize(&borrow_user, &market, 2000).await?;
    harness.borrow(&borrow_user, &market, 100).await?;
    assert_eq!(
        u128::from(
            harness
                .get_borrow_position(&market, &borrow_user.0)
                .await?
                .context("borrow position missing")?
                .get_total_borrow_asset_liability()
        ),
        100,
    );

    // Repaying 91 would drop the liability to 9, below the minimum of 10; the
    // market caps the repayment so the liability stays at the minimum.
    harness.repay(&borrow_user, &market, 91, None).await?;
    assert_eq!(
        u128::from(
            harness
                .get_borrow_position(&market, &borrow_user.0)
                .await?
                .context("borrow position missing")?
                .get_total_borrow_asset_liability()
        ),
        10,
    );

    Ok(())
}
