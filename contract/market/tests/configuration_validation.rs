use test_utils::*;

use templar_common::{
    dec,
    fee::{Fee, TimeBasedFee, TimeBasedFeeFunction},
    interest_rate_strategy::InterestRateStrategy,
};

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_asset`: must not equal `collateral_asset`"]
async fn borrow_asset_is_collateral_asset() {
    let worker = near_workspaces::sandbox().await.unwrap();
    setup_everything(
        &worker,
        |c| {
            c.borrow_asset = c.collateral_asset.clone().coerce();
        },
        |_c| {},
    )
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_interest_rate_strategy`: out of bounds"]
async fn borrow_interest_rate_strategy_exceed_apy_limit() {
    let worker = near_workspaces::sandbox().await.unwrap();
    setup_everything(
        &worker,
        |c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("0"), dec!("100001")).unwrap();
        },
        |_c| {},
    )
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_mcr_maintenance`: out of bounds"]
async fn borrow_mcr_maintenance_less_than_1() {
    let worker = near_workspaces::sandbox().await.unwrap();
    setup_everything(
        &worker,
        |c| {
            c.borrow_mcr_maintenance = dec!(".99");
        },
        |_c| {},
    )
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_mcr_maintenance`: out of bounds"]
async fn borrow_mcr_maintenance_less_than_borrow_mcr_liquidation() {
    let worker = near_workspaces::sandbox().await.unwrap();
    setup_everything(
        &worker,
        |c| {
            c.borrow_mcr_maintenance = dec!("1.2");
            c.borrow_mcr_liquidation = dec!("1.200000001");
        },
        |_c| {},
    )
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_mcr_liquidation`: out of bounds"]
async fn borrow_mcr_liquidation_less_than_1() {
    let worker = near_workspaces::sandbox().await.unwrap();
    setup_everything(
        &worker,
        |c| {
            c.borrow_mcr_liquidation = dec!(".99");
        },
        |_c| {},
    )
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_asset_maximum_usage_ratio`: out of bounds"]
async fn borrow_asset_maximum_usage_ratio_is_zero() {
    let worker = near_workspaces::sandbox().await.unwrap();
    setup_everything(
        &worker,
        |c| {
            c.borrow_asset_maximum_usage_ratio = dec!("0");
        },
        |_c| {},
    )
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_asset_maximum_usage_ratio`: out of bounds"]
async fn borrow_asset_maximum_usage_ratio_greater_than_1() {
    let worker = near_workspaces::sandbox().await.unwrap();
    setup_everything(
        &worker,
        |c| {
            c.borrow_asset_maximum_usage_ratio = dec!("1.0001");
        },
        |_c| {},
    )
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `supply_withdrawal_range.minimum`: out of bounds"]
async fn withdrawal_minimum_greater_than_supply_minimum() {
    let worker = near_workspaces::sandbox().await.unwrap();
    setup_everything(
        &worker,
        |c| {
            c.supply_range = (1, None).try_into().unwrap();
            c.supply_withdrawal_range = (2, None).try_into().unwrap();
        },
        |_c| {},
    )
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `supply_withdrawal_fee.fee`: out of bounds"]
async fn withdrawal_fee_greater_than_withdrawal_minimum() {
    let worker = near_workspaces::sandbox().await.unwrap();
    setup_everything(
        &worker,
        |c| {
            c.supply_range = (2, None).try_into().unwrap();
            c.supply_withdrawal_range = (2, None).try_into().unwrap();
            c.supply_withdrawal_fee = TimeBasedFee {
                fee: Fee::Flat(100.into()),
                duration: 100.into(),
                behavior: TimeBasedFeeFunction::Linear,
            };
        },
        |_c| {},
    )
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `liquidation_maximum_spread`: out of bounds"]
async fn liquidation_maximum_spread_greater_than_1() {
    let worker = near_workspaces::sandbox().await.unwrap();
    setup_everything(
        &worker,
        |c| {
            c.liquidation_maximum_spread = dec!("2");
        },
        |_c| {},
    )
    .await;
}
