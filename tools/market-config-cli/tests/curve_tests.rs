use market_config_cli::curve::{strategy_from_name, CurveInput, ModelArg};
use rstest::rstest;
use templar_common::number::Decimal;

#[rstest]
#[case(ModelArg::Piecewise, "piecewise")]
#[case(ModelArg::Linear, "linear")]
#[case(ModelArg::Exponential, "exponential")]
fn model_arg_as_str_matches_expected(#[case] model: ModelArg, #[case] expected: &str) {
    assert_eq!(model.as_str(), expected);
}

#[rstest]
#[case("linear")]
#[case("piecewise")]
#[case("exponential")]
fn strategy_from_name_accepts_known_models(#[case] name: &str) {
    assert!(
        strategy_from_name(name).is_ok(),
        "expected {name} to succeed"
    );
}

#[test]
fn strategy_from_name_rejects_unknown_model() {
    let err = strategy_from_name("unknown-model").unwrap_err();
    assert!(err.to_string().contains("Unknown model"));
}

#[test]
fn any_flag_provided_reflects_presence() {
    let empty = CurveInput {
        starting_rate: None,
        optimal_rate: None,
        optimal_usage: None,
        max_rate: None,
        display_points: 10,
        model: None,
        eccentricity: None,
    };
    assert!(!empty.any_flag_provided(), "no flags should report false");

    let with_one_flag = CurveInput {
        starting_rate: Some(Decimal::ZERO),
        ..empty
    };
    assert!(
        with_one_flag.any_flag_provided(),
        "any flag should report true"
    );
}
