use market_config_cli::{config::ConfigBuilder, ui::prompt::parsers::parse_price_id, CliError};
use std::str::FromStr;
use templar_common::{
    fee::Fee, interest_rate_strategy::InterestRateStrategy, market::YieldWeights, number::Decimal,
};

#[test]
fn parse_price_id_rejects_bad_input() {
    let too_short = parse_price_id("1234");
    assert!(matches!(too_short, Err(CliError::InvalidInput(_))));

    let bad_hex =
        parse_price_id("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz");
    assert!(matches!(bad_hex, Err(CliError::InvalidInput(_))));
}

#[test]
fn parse_price_id_accepts_valid_hex() {
    let hex = "c415de3e590e4fa0b0e05d4c2e3e3f3e0df9c2d3e08f920d3c7e6b0e7e7b4e0a";
    let parsed = parse_price_id(hex).expect("valid price id should parse");
    assert_eq!(parsed.0.len(), 32);
}

#[test]
fn config_builder_happy_path_builds() {
    let strategy = InterestRateStrategy::linear(
        Decimal::from_str("0.05").unwrap(),
        Decimal::from_str("0.10").unwrap(),
    )
    .expect("linear strategy");

    let config = ConfigBuilder::new()
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
        .borrow_interest_rate_strategy(strategy)
        .borrow_max_duration_ms(None)
        .borrow_range(1_000_000, None)
        .unwrap()
        .supply_range(1_000_000, None)
        .unwrap()
        .supply_withdrawal_range(1_000_000, None)
        .unwrap()
        .supply_withdrawal_fee(templar_common::fee::TimeBasedFee::zero())
        .yield_weights(YieldWeights::new_with_supply_weight(9))
        .protocol_account_id("protocol.near")
        .unwrap()
        .liquidation_max_spread(Decimal::from_str("0.05").unwrap())
        .build();

    assert!(config.is_ok(), "config should build with valid inputs");
}

#[test]
fn config_builder_rejects_invalid_ranges() {
    let builder = ConfigBuilder::new()
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
        .borrow_max_duration_ms(None);

    // borrow_range with max < min should fail
    let result = builder.clone().borrow_range(2, Some(1));
    assert!(result.is_err(), "borrow_range should reject max < min");

    let builder = builder
        .borrow_range(1, None)
        .unwrap()
        .supply_range(1, None)
        .unwrap();

    // withdrawal range with min above supply max should be rejected by try_into
    let withdrawal_err = builder.supply_withdrawal_range(30, Some(20));
    assert!(
        withdrawal_err.is_err(),
        "withdrawal range should validate bounds"
    );
}
