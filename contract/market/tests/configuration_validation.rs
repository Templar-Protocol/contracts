use test_utils::*;

use templar_common::{dec, interest_rate_strategy::InterestRateStrategy};

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_asset`: must not equal `collateral_asset`"]
async fn borrow_asset_is_collateral_asset() {
    setup_everything(|c| {
        c.borrow_asset = c.collateral_asset.clone().coerce();
    })
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_interest_rate_strategy`: out of bounds"]
async fn borrow_interest_rate_strategy_exceed_apy_limit() {
    setup_everything(|c| {
        c.borrow_interest_rate_strategy =
            InterestRateStrategy::linear(dec!("0"), dec!("100001")).unwrap();
    })
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_mcr_initial`: out of bounds"]
async fn borrow_mcr_initial_less_than_1() {
    setup_everything(|c| {
        c.borrow_mcr_initial = dec!(".99");
    })
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_mcr_initial`: out of bounds"]
async fn borrow_mcr_initial_less_than_borrow_mcr() {
    setup_everything(|c| {
        c.borrow_mcr_initial = dec!("1.2");
        c.borrow_mcr = dec!("1.200000001");
    })
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_mcr`: out of bounds"]
async fn borrow_mcr_less_than_1() {
    setup_everything(|c| {
        c.borrow_mcr = dec!(".99");
    })
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_asset_maximum_usage_ratio`: out of bounds"]
async fn borrow_asset_maximum_usage_ratio_is_zero() {
    setup_everything(|c| {
        c.borrow_asset_maximum_usage_ratio = dec!("0");
    })
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `borrow_asset_maximum_usage_ratio`: out of bounds"]
async fn borrow_asset_maximum_usage_ratio_greater_than_1() {
    setup_everything(|c| {
        c.borrow_asset_maximum_usage_ratio = dec!("1.0001");
    })
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `supply_withdrawal_range.minimum`: out of bounds"]
async fn withdrawal_minimum_greater_than_supply_minimum() {
    setup_everything(|c| {
        c.supply_range = (1, None).try_into().unwrap();
        c.supply_withdrawal_range = (2, None).try_into().unwrap();
    })
    .await;
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Invalid configuration field `liquidation_maximum_spread`: out of bounds"]
async fn liquidation_maximum_spread_greater_than_1() {
    setup_everything(|c| {
        c.liquidation_maximum_spread = dec!("2");
    })
    .await;
}
