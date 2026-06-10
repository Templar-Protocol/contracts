use market_config_cli::config::ConfigBuilder;
use near_sdk::AccountId;
use rstest::rstest;
use std::str::FromStr;
use templar_common::{
    fee::Fee, interest_rate_strategy::InterestRateStrategy, market::YieldWeights, Decimal,
};

#[allow(clippy::unwrap_used)]
fn fully_populated_builder() -> ConfigBuilder {
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
                Decimal::from_str("0.05").unwrap(),
                Decimal::from_str("0.10").unwrap(),
            )
            .unwrap(),
        )
        .borrow_max_duration_ms(None)
        .borrow_range(1, None)
        .unwrap()
        .supply_range(1, None)
        .unwrap()
        .supply_withdrawal_range(1, None)
        .unwrap()
        .supply_withdrawal_fee(templar_common::fee::TimeBasedFee::zero())
        .yield_weights(templar_common::market::YieldWeights::new_with_supply_weight(10))
        .protocol_account_id("protocol.near")
        .unwrap()
        .liquidation_max_spread(Decimal::from_str("0.05").unwrap())
}

#[test]
fn builder_happy_path_builds() {
    let config = fully_populated_builder()
        .build()
        .expect("fully populated builder should succeed");
    assert_eq!(config.price_oracle_configuration.price_maximum_age_s, 60);
}

#[test]
fn builder_supports_multiple_static_yield_recipients() {
    let weights = YieldWeights::new_with_supply_weight(8)
        .with_static(AccountId::from_str("revenue.tmplr.near").unwrap(), 1)
        .with_static(AccountId::from_str("ops.tmplr.near").unwrap(), 1);

    let mut builder = fully_populated_builder();
    builder = builder.yield_weights(weights.clone());

    let config = builder.build().expect("config should build");
    assert_eq!(config.yield_weights.r#static.len(), 2);
    assert_eq!(
        config
            .yield_weights
            .r#static
            .get(&AccountId::from_str("revenue.tmplr.near").unwrap()),
        weights
            .r#static
            .get(&AccountId::from_str("revenue.tmplr.near").unwrap())
    );
}

#[rstest]
#[case::missing_time_chunk(ConfigBuilder::new(), "time_chunk_duration_ms is required")]
#[case::missing_assets(
    ConfigBuilder::new().time_chunk_duration_ms(600_000),
    "borrow_asset is required"
)]
#[case::missing_oracle(
    ConfigBuilder::new()
        .time_chunk_duration_ms(600_000)
        .borrow_asset("usdc.near")
        .unwrap()
        .collateral_asset("wnear.near")
        .unwrap(),
    "oracle_account_id is required"
)]
fn builder_missing_required_fields(#[case] builder: ConfigBuilder, #[case] expected_message: &str) {
    let err = builder.build().unwrap_err();
    assert!(
        err.to_string().contains(expected_message),
        "expected error to mention `{expected_message}`, got {err}"
    );
}
