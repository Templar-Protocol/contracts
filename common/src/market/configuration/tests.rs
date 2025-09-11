use near_sdk::{
    json_types::U128,
    serde_json::{self, json},
};
use rstest::rstest;

use crate::{borrow::InterestAccumulationProof, dec, oracle::pyth};

use super::*;

#[test]
fn test_satisfies_minimum_collateral_ratio() {
    let mut b = BorrowPosition::new(0);
    b.increase_collateral_asset_deposit(121u128.into());
    b.increase_borrow_asset_principal(InterestAccumulationProof::test(), 100u128.into(), 0);
    assert!(satisfies_minimum_collateral_ratio(
        dec!("1.2"),
        &b,
        &PricePair::new(
            &pyth::Price {
                price: near_sdk::json_types::I64(10000),
                conf: U64(1),
                expo: -4,
                publish_time: 0,
            },
            18,
            &pyth::Price {
                price: near_sdk::json_types::I64(10000),
                conf: U64(1),
                expo: -4,
                publish_time: 0,
            },
            18,
        )
        .unwrap()
    ));
}

#[rstest]
#[case(1, 0)]
#[case(0, 0)]
#[case(u128::MAX, 0)]
#[case(u128::MAX, u128::MAX - 1)]
#[case(500, 10)]
#[should_panic = "Invalid range specified"]
fn invalid_amount_range(#[case] min: u128, #[case] max: u128) {
    ValidAmountRange::<BorrowAsset>::try_from((min, Some(max))).unwrap();
}

#[rstest]
#[case(1, 0)]
#[case(0, 0)]
#[case(u128::MAX, 0)]
#[case(u128::MAX, u128::MAX - 1)]
#[case(500, 10)]
#[should_panic = "Invalid range specified"]
fn invalid_amount_range_json(#[case] min: u128, #[case] max: u128) {
    serde_json::from_value::<ValidAmountRange<BorrowAsset>>(json!({
        "minimum": U128(min),
        "maximum": U128(max),
    }))
    .unwrap();
}

#[rstest]
#[case(1, 1)]
#[case(0, u128::MAX)]
#[case(1, u128::MAX)]
#[case(u128::MAX, u128::MAX)]
#[case(u128::MAX - 1, u128::MAX)]
#[case(10, 500)]
fn valid_amount_range(#[case] min: u128, #[case] max: u128) {
    ValidAmountRange::<BorrowAsset>::try_from((min, Some(max))).unwrap();
}

#[rstest]
#[case(1, 1)]
#[case(0, u128::MAX)]
#[case(1, u128::MAX)]
#[case(u128::MAX, u128::MAX)]
#[case(u128::MAX - 1, u128::MAX)]
#[case(10, 500)]
fn valid_amount_range_json(#[case] min: u128, #[case] max: u128) {
    serde_json::from_value::<ValidAmountRange<BorrowAsset>>(json!({
        "minimum": U128(min),
        "maximum": U128(max),
    }))
    .unwrap();
}
