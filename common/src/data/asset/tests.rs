use crate::asset_op;
use near_sdk::serde_json;

use super::*;

#[test]
fn serialization() {
    let amount = BorrowAssetAmount::new(100);
    let serialized = serde_json::to_string(&amount).unwrap();
    assert_eq!(serialized, "\"100\"");
    let deserialized: BorrowAssetAmount = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized, amount);
}

#[test]
#[should_panic = "a + u128::MAX overflow"]
fn asset_op_macro_overflow() {
    let mut a = BorrowAssetAmount::new(100);

    asset_op! {
        a += u128::MAX;
    };
}

#[test]
#[should_panic = "a - 101u128 underflow"]
fn asset_op_macro_underflow() {
    let mut a = BorrowAssetAmount::new(100);

    asset_op! {
        a -= 101u128;
    };
}
