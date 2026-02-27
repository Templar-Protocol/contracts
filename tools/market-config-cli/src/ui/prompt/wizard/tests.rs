use super::assets::asset_defaults;
use super::types::AssetStandard;
use crate::ui::prompt::ranges::{apply_ranges_to_builder, RangeSelection};
use crate::ConfigBuilder;
use near_sdk::AccountId;
use rstest::rstest;
use std::str::FromStr;
use templar_common::{
    asset::{BorrowAsset, FungibleAsset},
    fee::Fee,
    interest_rate_strategy::InterestRateStrategy,
    market::YieldWeights,
    number::Decimal,
};

fn base_builder() -> ConfigBuilder {
    ConfigBuilder::new()
        .time_chunk_duration_ms(600_000)
        .borrow_asset("usdc.near")
        .unwrap()
        .collateral_asset("wnear.near")
        .unwrap()
        .oracle_account_id("pyth-oracle.near")
        .unwrap()
        .borrow_price_id([0xbb; 32])
        .borrow_decimals(6)
        .collateral_price_id([0xaa; 32])
        .collateral_decimals(24)
        .price_max_age_s(60)
        .borrow_mcr_maintenance(Decimal::from_str("1.25").unwrap())
        .borrow_mcr_liquidation(Decimal::from_str("1.20").unwrap())
        .borrow_max_usage_ratio(Decimal::from_str("0.90").unwrap())
        .borrow_origination_fee(Fee::zero())
        .borrow_interest_rate_strategy(
            InterestRateStrategy::linear(
                Decimal::from_str("0.01").unwrap(),
                Decimal::from_str("0.10").unwrap(),
            )
            .unwrap(),
        )
        .borrow_max_duration_ms(None)
        .supply_withdrawal_fee(templar_common::fee::TimeBasedFee::zero())
        .yield_weights(YieldWeights::new_with_supply_weight(9))
        .protocol_account_id("protocol.near")
        .unwrap()
        .liquidation_max_spread(Decimal::from_str("0.05").unwrap())
}

#[rstest]
#[case(
    FungibleAsset::<BorrowAsset>::nep141("usdc.near".parse().unwrap()),
    AssetStandard::Nep141,
    "usdc.near",
    None
)]
#[case(
    FungibleAsset::<BorrowAsset>::nep245("mt.near".parse().unwrap(), "btc-token".to_string()),
    AssetStandard::Nep245,
    "mt.near",
    Some("btc-token")
)]
fn asset_defaults_handles_assets(
    #[case] asset: FungibleAsset<BorrowAsset>,
    #[case] expected_standard: AssetStandard,
    #[case] expected_contract: &str,
    #[case] expected_token: Option<&str>,
) {
    let (standard, contract, token) = asset_defaults(&asset);
    assert!(
        matches!(standard, s if matches!(expected_standard, AssetStandard::Nep141) && matches!(s, AssetStandard::Nep141)
        || matches!(expected_standard, AssetStandard::Nep245) && matches!(s, AssetStandard::Nep245))
    );
    assert_eq!(contract, expected_contract);
    assert_eq!(token.as_deref(), expected_token);
}

#[rstest]
#[case(Some(10), Some(20), Some(30))]
#[case(None, None, None)]
#[case(Some(50), None, Some(40))]
fn apply_ranges_to_builder_respects_withdrawal_max(
    #[case] borrow_max: Option<u128>,
    #[case] supply_max: Option<u128>,
    #[case] withdrawal_max: Option<u128>,
) {
    let selection = RangeSelection {
        borrow_min: 1,
        borrow_max,
        supply_min: 2,
        supply_max,
        withdrawal_min: 3,
        withdrawal_max,
    };

    let builder = apply_ranges_to_builder(base_builder(), &selection)
        .expect("range application should succeed");
    let config = builder.build().expect("config should build");

    assert_eq!(
        config.borrow_range.maximum.map(u128::from),
        selection.borrow_max
    );
    assert_eq!(
        config.supply_range.maximum.map(u128::from),
        selection.supply_max
    );
    assert_eq!(
        config.supply_withdrawal_range.maximum.map(u128::from),
        selection.withdrawal_max
    );
}

#[test]
fn apply_ranges_to_builder_happy_path_sets_all_minimums() {
    let selection = RangeSelection {
        borrow_min: 1_000_000,
        borrow_max: Some(100_000_000),
        supply_min: 500_000,
        supply_max: Some(50_000_000),
        withdrawal_min: 100_000,
        withdrawal_max: Some(10_000_000),
    };

    let builder = apply_ranges_to_builder(base_builder(), &selection)
        .expect("range application should succeed");
    let config = builder.build().expect("config should build");

    // Verify all minimums are correctly set
    assert_eq!(
        u128::from(config.borrow_range.minimum),
        selection.borrow_min
    );
    assert_eq!(
        u128::from(config.supply_range.minimum),
        selection.supply_min
    );
    assert_eq!(
        u128::from(config.supply_withdrawal_range.minimum),
        selection.withdrawal_min
    );

    // Verify all maximums are correctly set
    assert_eq!(
        config.borrow_range.maximum.map(u128::from),
        selection.borrow_max
    );
    assert_eq!(
        config.supply_range.maximum.map(u128::from),
        selection.supply_max
    );
    assert_eq!(
        config.supply_withdrawal_range.maximum.map(u128::from),
        selection.withdrawal_max
    );
}

#[test]
fn yield_weights_happy_path_calculates_shares_correctly() {
    // Test that yield weights distribute correctly between suppliers and static recipients
    let supply_weight = 9u16;
    let weights = YieldWeights::new_with_supply_weight(supply_weight);

    // Initial state: only supplier weight
    assert_eq!(weights.supply.get(), supply_weight);
    assert!(weights.r#static.is_empty());
    assert_eq!(u16::from(weights.total_weight()), supply_weight);

    // Add a static recipient
    let revenue_account: near_sdk::AccountId = "revenue.near".parse().unwrap();
    let weights_with_static = weights.with_static(revenue_account.clone(), 1);
    assert_eq!(weights_with_static.supply.get(), supply_weight);
    assert_eq!(u16::from(weights_with_static.total_weight()), 10); // 9 + 1
    assert_eq!(weights_with_static.r#static.get(&revenue_account), Some(&1));

    // Verify percentage calculation (suppliers get 90%, static gets 10%)
    let total = f64::from(u16::from(weights_with_static.total_weight()));
    let supplier_share = f64::from(supply_weight) / total * 100.0;
    assert!((supplier_share - 90.0).abs() < 0.01);
}

#[test]
fn yield_weights_multiple_static_recipients() {
    let supply_weight = 8u16;
    let revenue: AccountId = "revenue.near".parse().unwrap();
    let treasury: AccountId = "treasury.near".parse().unwrap();
    let weights = YieldWeights::new_with_supply_weight(supply_weight)
        .with_static(revenue, 1)
        .with_static(treasury, 1);

    // Total should be 8 + 1 + 1 = 10
    assert_eq!(u16::from(weights.total_weight()), 10);
    assert_eq!(weights.r#static.len(), 2);

    // Suppliers get 80%, each static recipient gets 10%
    let total = f64::from(u16::from(weights.total_weight()));
    let supplier_share = f64::from(supply_weight) / total * 100.0;
    assert!((supplier_share - 80.0).abs() < 0.01);
}

#[test]
fn full_config_build_happy_path_with_ranges_and_yield_weights() {
    let selection = RangeSelection {
        borrow_min: 1_000_000,               // 1 USDC (6 decimals)
        borrow_max: Some(1_000_000_000_000), // 1M USDC
        supply_min: 1_000_000,
        supply_max: None,        // No max
        withdrawal_min: 100_000, // 0.1 USDC
        withdrawal_max: None,
    };

    let protocol: AccountId = "protocol.near".parse().unwrap();
    let yield_weights = YieldWeights::new_with_supply_weight(9).with_static(protocol, 1);

    let builder = base_builder().yield_weights(yield_weights);
    let builder =
        apply_ranges_to_builder(builder, &selection).expect("range application should succeed");
    let config = builder.build().expect("config should build");

    // Verify the complete config is valid
    assert_eq!(u128::from(config.borrow_range.minimum), 1_000_000);
    assert_eq!(
        config.borrow_range.maximum.map(u128::from),
        Some(1_000_000_000_000)
    );
    assert_eq!(u128::from(config.supply_range.minimum), 1_000_000);
    assert!(config.supply_range.maximum.is_none());
    assert_eq!(u128::from(config.supply_withdrawal_range.minimum), 100_000);
    assert!(config.supply_withdrawal_range.maximum.is_none());

    // Verify yield weights
    assert_eq!(config.yield_weights.supply.get(), 9);
    assert_eq!(u16::from(config.yield_weights.total_weight()), 10);
}
