//! Ported from `contract/market/tests/supply_maximum_amount.rs`.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::time_chunk::TimeChunkConfiguration;
use templar_gateway_testing::{harness, SandboxHarness};
use templar_gateway_types::OperationStatus;

#[rstest]
#[case(&[10_000], 10_000)]
#[case(&[1_000, 9_000], 10_000)]
#[case(&[1; 25], 10_000)]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn supply_within_maximum(
    #[future(awt)] harness: SandboxHarness,
    #[case] deposits: &[u128],
    #[case] supply_maximum: u128,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.supply_range = (1, Some(supply_maximum)).try_into().unwrap();
            c.time_chunk_configuration = TimeChunkConfiguration::new(1000 * 20);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    harness.fund_user(&supply_user, &market).await?;

    let mut sum = 0;
    for &deposit in deposits {
        sum += deposit;
        harness.supply(&supply_user, &market, deposit).await?;
    }

    let supply_position = harness
        .get_supply_position(&market, &supply_user.0)
        .await?
        .context("supply position missing")?;
    assert_eq!(u128::from(supply_position.get_deposit().total()), sum);

    Ok(())
}

#[rstest]
#[case(&[10_001], 10_000)]
#[case(&[1, 100_000], 10_000)]
#[case(&[9_001, 500, 500], 10_000)]
#[case(&[2], 1)]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn supply_beyond_maximum(
    #[future(awt)] harness: SandboxHarness,
    #[case] deposits: &[u128],
    #[case] supply_maximum: u128,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.supply_range = (1, Some(supply_maximum)).try_into().unwrap();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    harness.fund_user(&supply_user, &market).await?;

    // Every case is constructed so the running total only exceeds the maximum on
    // the final deposit; the earlier ones succeed.
    let (last, leading) = deposits.split_last().expect("at least one deposit");
    let mut leading_sum = 0;
    for &deposit in leading {
        leading_sum += deposit;
        harness.supply(&supply_user, &market, deposit).await?;
    }

    // The market rejects the over-maximum deposit inside `ft_on_transfer`; the FT
    // catches that panic and refunds, so the `ft_transfer_call` operation itself
    // succeeds while the supply position is left unchanged.
    harness.try_supply(&supply_user, &market, *last).await?;
    let recorded = harness
        .get_supply_position(&market, &supply_user.0)
        .await?
        .map_or(0, |position| u128::from(position.get_deposit().total()));
    assert_eq!(
        recorded, leading_sum,
        "the over-maximum deposit must be refunded, not recorded",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn harvest_yield_beyond_maximum(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    const LIMIT: u128 = 1_000_000;
    let market = harness
        .deploy_full_market_with(|c| {
            c.supply_range = (LIMIT, Some(LIMIT)).try_into().unwrap();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, LIMIT)
        .await?;
    harness
        .collateralize(&borrow_user, &market, LIMIT * 2)
        .await?;

    harness.borrow(&borrow_user, &market, LIMIT * 4 / 5).await?;
    harness.repay(&borrow_user, &market, LIMIT, None).await?;

    // No longer a compounding operation, so harvesting back into a maxed-out
    // position is fine.
    let result = harness
        .harvest_yield(&supply_user, &market, Some(supply_user.0.clone()))
        .await?;
    assert_eq!(result.operation.status, OperationStatus::Succeeded);

    Ok(())
}
