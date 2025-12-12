use market_config_cli::editor::utils::{
    fee_defaults, parse_asset_input, price_id_from_input, StrategyDefaults, StrategyKind,
};
use near_sdk::AccountId;
use rstest::rstest;
use std::str::FromStr;
use templar_common::{
    asset::BorrowAsset, fee::Fee, interest_rate_strategy::InterestRateStrategy, number::Decimal,
};

#[rstest]
#[case("usdc.near")]
#[case("wrap.near")]
fn parse_asset_input_accepts_valid_accounts(#[case] account: &str) {
    let asset = parse_asset_input::<BorrowAsset>(account, "borrow asset")
        .expect("valid account should parse as asset");
    assert_eq!(asset.contract_id(), &AccountId::from_str(account).unwrap());
}

#[rstest]
#[case("")]
#[case("not a valid account")]
fn parse_asset_input_rejects_invalid_accounts(#[case] account: &str) {
    let err = parse_asset_input::<BorrowAsset>(account, "borrow asset").unwrap_err();
    assert!(
        err.to_string().contains("Invalid borrow asset"),
        "unexpected error: {err}"
    );
}

#[rstest]
#[case("b7a8eba68a997cd0210c2e1e4ee811ad2d174b3611c22d9ebf16f4cb7e9ba850")]
#[case("0x70f9b53410a4ec4b6d9eae77a0f9bb6b6f2b12ed063e51252b52376c0f9a0001")]
fn price_id_from_input_accepts_valid_hex(#[case] hex: &str) {
    let parsed = price_id_from_input(hex).expect("valid price id should parse");
    assert_eq!(parsed.0.len(), 32);
}

#[rstest]
#[case("too-short")]
#[case("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz")]
fn price_id_from_input_rejects_bad_hex(#[case] hex: &str) {
    assert!(price_id_from_input(hex).is_err());
}

#[rstest]
#[case(
    Fee::Flat(templar_common::asset::FungibleAssetAmount::<BorrowAsset>::new(1)),
    (0, "1")
)]
#[case(Fee::Proportional(Decimal::from_str("0.05").unwrap()), (1, "0.05"))]
fn fee_defaults_extracts_mode_and_value(
    #[case] fee: Fee<BorrowAsset>,
    #[case] expected: (usize, &str),
) {
    let defaults = fee_defaults(&fee);
    assert_eq!(defaults.0, expected.0);
    assert_eq!(defaults.1, expected.1);
}

#[rstest]
#[case::linear(
    InterestRateStrategy::linear(Decimal::from_str("0.01").unwrap(), Decimal::from_str("0.02").unwrap()).unwrap(),
    StrategyKind::Linear.as_index(),
    &["base", "top"]
)]
#[case::piecewise(
    InterestRateStrategy::piecewise(
        Decimal::from_str("0.01").unwrap(),
        Decimal::from_str("0.80").unwrap(),
        Decimal::from_str("0.10").unwrap(),
        Decimal::from_str("0.25").unwrap()
    ).unwrap(),
    StrategyKind::Piecewise.as_index(),
    &["base", "optimal", "rate_1", "rate_2"]
)]
#[case::exponential(
    InterestRateStrategy::exponential2(
        Decimal::from_str("0.01").unwrap(),
        Decimal::from_str("0.50").unwrap(),
        Decimal::from_str("2").unwrap()
    ).unwrap(),
    StrategyKind::Exponential2.as_index(),
    &["base", "top", "eccentricity"]
)]
fn strategy_defaults_round_trip(
    #[case] strategy: InterestRateStrategy,
    #[case] expected_index: usize,
    #[case] expected_keys: &[&str],
) {
    let defaults = StrategyDefaults::from_strategy(&strategy).expect("strategy should serialize");
    assert_eq!(defaults.kind.as_index(), expected_index);

    for key in expected_keys {
        let value = defaults.get(key, "missing");
        assert_ne!(value, "missing", "expected key {key} to be present");
    }

    let fallback = defaults.get("not-a-key", "fallback");
    assert_eq!(fallback, "fallback");
}
