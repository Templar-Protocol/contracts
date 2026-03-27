#![no_main]

use libfuzzer_sys::fuzz_target;
use near_sdk::json_types::{I64, U64};
use templar_common::asset::{BorrowAssetAmount, CollateralAssetAmount};
use templar_common::oracle::pyth::{self, PythTimestamp};
use templar_common::price::{Appraise, Convert, PricePair};

fuzz_target!(|data: (i64, u64, i64, u64, i32, i32, u128, u128)| {
    let (
        collateral_price_raw,
        collateral_conf,
        borrow_price_raw,
        borrow_conf,
        collateral_decimals,
        borrow_decimals,
        collateral_amount,
        borrow_amount,
    ) = data;

    // Create Pyth price structs
    let collateral_pyth_price = pyth::Price {
        price: I64(collateral_price_raw),
        conf: U64(collateral_conf),
        expo: -8, // typical exponent
        publish_time: PythTimestamp::from_secs(0),
    };

    let borrow_pyth_price = pyth::Price {
        price: I64(borrow_price_raw),
        conf: U64(borrow_conf),
        expo: -8,
        publish_time: PythTimestamp::from_secs(0),
    };

    // Fuzz PricePair creation
    if let Ok(price_pair) = PricePair::new(
        &collateral_pyth_price,
        collateral_decimals,
        &borrow_pyth_price,
        borrow_decimals,
    ) {
        // Fuzz valuation for collateral
        let collateral_amt = CollateralAssetAmount::new(collateral_amount);
        let _ = price_pair.valuation(collateral_amt);

        // Fuzz valuation for borrow
        let borrow_amt = BorrowAssetAmount::new(borrow_amount);
        let _ = price_pair.valuation(borrow_amt);

        // Fuzz conversions
        let _ = price_pair.convert(collateral_amt);
        let _ = price_pair.convert(borrow_amt);

        // Fuzz Valuation::ratio
        let val1 = price_pair.valuation(collateral_amt);
        let val2 = price_pair.valuation(borrow_amt);
        let _ = val1.ratio(val2);

        // Test edge cases
        let zero_collateral = CollateralAssetAmount::zero();
        let zero_borrow = BorrowAssetAmount::zero();
        let _ = price_pair.valuation(zero_collateral);
        let _ = price_pair.valuation(zero_borrow);
    }

    // Fuzz with different exponents
    let varying_expo_price = pyth::Price {
        price: I64(collateral_price_raw),
        conf: U64(collateral_conf),
        expo: collateral_decimals.wrapping_sub(borrow_decimals),
        publish_time: PythTimestamp::from_secs(0),
    };

    let _ = PricePair::new(
        &varying_expo_price,
        collateral_decimals,
        &borrow_pyth_price,
        borrow_decimals,
    );
});
