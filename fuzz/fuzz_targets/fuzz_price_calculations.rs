//! Fuzz the real `Convert` implementations on `PricePair` (`common/src/price.rs`)
//! with a **round-trip / reciprocal** oracle (P1: real code, independent
//! oracle).
//!
//! Scope is deliberately complementary to `fuzz_price`: that target covers
//! `valuation`/`ratio` monotonicity and the ENG-343 extreme-exponent boundary;
//! this one drives both `convert` directions and asserts the *metamorphic*
//! property that converting a unit of value one way and back must recover that
//! unit — a property that holds independently of the conversion formula, so it
//! catches a transposed numerator/denominator, a swapped price, or a dropped
//! scaling that the per-direction monotonicity checks cannot see.
//!
//! Inputs are Pyth-shaped and kept in a realistic range (P2, targeted +
//! documented): prices in `[1e5, ~1e9]`, decimals in `[0, 18]`, and exponents
//! in `[-12, -4]`. The exponent is **fuzzed**, not fixed, so the exponent-gap
//! axis is exercised — but bounded together with decimals to stay clear of the
//! extreme-gap region that trips the intentional U512 overflow / ENG-343 div0
//! (owned by `fuzz_price`). The combined gap stays within `mul_pow10`'s exact
//! range, so `ratio` takes its exact path and the round-trip identity holds to
//! Decimal precision.

#![no_main]
#![cfg(not(target_arch = "wasm32"))]
#![allow(
    clippy::expect_used,
    reason = "panics on invariant violation are the intended libFuzzer crash signal"
)]

use libfuzzer_sys::fuzz_target;
use near_sdk::json_types::{I64, U64};
use templar_common::{
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAsset, CollateralAssetAmount},
    oracle::pyth::{self, PythTimestamp},
    price::{Appraise, Convert, PricePair},
    Decimal,
};

// MUTATION-CHECK (P5): in `Convert<CollateralAsset, BorrowAsset>::convert`
// (price.rs:111), swap the `ratio` operands (`valuation(1 borrow).ratio(
// valuation(amount))`). The forward/back product then diverges from 1 and the
// round-trip assertion below must fire. (Alternatively: delete the
// `if rhs.coefficient.is_zero() { return None; }` guard in `Valuation::ratio`
// to break the zero-valuation assertion.)

fuzz_target!(|data: (i64, u64, i64, u64, u64, u64, i32, i32, i32, i32)| {
    let (c_px, c_conf, b_px, b_conf, coll_amt, borrow_amt, c_dec, b_dec, c_expo_raw, b_expo_raw) =
        data;

    // Pyth-realistic: prices in [1e5, ~1e9], conf < price, decimals in [0,18],
    // exponents in [-12,-4] (fuzzed). See the module doc for why these bounds
    // are targeted (not a blanket narrow) and how they keep `ratio` exact.
    let c_price = 100_000 + (c_px.unsigned_abs() % 1_000_000_000);
    let b_price = 100_000 + (b_px.unsigned_abs() % 1_000_000_000);
    let c_conf_b = c_conf % c_price;
    let b_conf_b = b_conf % b_price;
    let c_dec = c_dec.rem_euclid(19);
    let b_dec = b_dec.rem_euclid(19);
    let c_expo = -4 - c_expo_raw.rem_euclid(9); // [-12, -4]
    let b_expo = -4 - b_expo_raw.rem_euclid(9); // [-12, -4]

    #[allow(
        clippy::cast_possible_wrap,
        reason = "price bounded to ~1e9, far inside i64 range"
    )]
    let c_pyth = pyth::Price {
        price: I64(c_price as i64),
        conf: U64(c_conf_b),
        expo: c_expo,
        publish_time: PythTimestamp::from_secs(0),
    };
    #[allow(
        clippy::cast_possible_wrap,
        reason = "price bounded to ~1e9, far inside i64 range"
    )]
    let b_pyth = pyth::Price {
        price: I64(b_price as i64),
        conf: U64(b_conf_b),
        expo: b_expo,
        publish_time: PythTimestamp::from_secs(0),
    };

    let Ok(price_pair) = PricePair::new(&c_pyth, c_dec, &b_pyth, b_dec) else {
        return;
    };

    let coll = CollateralAssetAmount::new(u128::from(coll_amt));
    let borrow = BorrowAssetAmount::new(u128::from(borrow_amt));

    // ---- Non-trivial properties of the real functions (P2) ----

    // 1. A zero-amount valuation has a zero coefficient, so using it as the
    //    divisor of `ratio` must be None (the div0 guard). HARD assertion: a
    //    wrong-but-nonzero result is not tolerated.
    let v_zero = price_pair.valuation(CollateralAssetAmount::zero());
    let v_one = price_pair.valuation(CollateralAssetAmount::new(1));
    assert!(
        v_one.ratio(v_zero).is_none(),
        "ratio by a zero-amount valuation must be None (div0 guard)",
    );

    // 2. Round-trip / reciprocal identity (the precision oracle). Converting one
    //    unit of borrow value into collateral units and one unit of collateral
    //    value into borrow units are reciprocals: their product must equal 1.
    //    The confidence terms cancel exactly (optimistic·pessimistic over
    //    pessimistic·optimistic), so this is a clean implementation-independent
    //    check. With the bounded domain both directions stay on `ratio`'s exact
    //    path, so the only error is Decimal truncation. The worst case is an
    //    extreme exponent gap that drives one direction to ~1e-30, which keeps
    //    only ~28 of Decimal's significant bits (~3e-9 relative error); we
    //    assert within 1e-6 — ~300× that worst case, yet far tighter than any
    //    *structural* bug (a transposed operand or wrong price shifts the
    //    product by O(1)).
    let fwd = <PricePair as Convert<BorrowAsset, CollateralAsset>>::convert(
        &price_pair,
        BorrowAssetAmount::new(1),
    );
    let back = <PricePair as Convert<CollateralAsset, BorrowAsset>>::convert(
        &price_pair,
        CollateralAssetAmount::new(1),
    );
    // Guard against a saturated ratio (Decimal::MAX / zero), which the bounded
    // domain should never produce but which would make the product meaningless.
    if !fwd.is_zero() && fwd != Decimal::MAX && !back.is_zero() && back != Decimal::MAX {
        let product = fwd * back;
        let tolerance = Decimal::ONE.mul_pow10(-6).expect("1e-6 is in range");
        assert!(
            product.abs_diff(Decimal::ONE) <= tolerance,
            "convert round-trip must recover unity: fwd={fwd:?} * back={back:?} = {product:?}",
        );
    }

    // 3. Monotonicity: a larger amount yields a larger-or-equal valuation.
    if u128::from(coll_amt) > 0 {
        let v_coll = price_pair.valuation(coll);
        let v_coll_plus = price_pair.valuation(CollateralAssetAmount::new(
            u128::from(coll_amt).saturating_add(1),
        ));
        if let Some(ratio) = v_coll_plus.ratio(v_coll) {
            assert!(
                ratio >= Decimal::ONE,
                "valuation must be monotone non-decreasing in amount; got ratio={ratio:?}",
            );
        }
    }
    if u128::from(borrow_amt) > 0 {
        let v_borrow = price_pair.valuation(borrow);
        let v_borrow_plus = price_pair.valuation(BorrowAssetAmount::new(
            u128::from(borrow_amt).saturating_add(1),
        ));
        if let Some(ratio) = v_borrow_plus.ratio(v_borrow) {
            assert!(
                ratio >= Decimal::ONE,
                "borrow valuation must be monotone in amount; got ratio={ratio:?}",
            );
        }
    }

    // 4. The real conversions on the fuzzed amounts must not panic.
    let _ = <PricePair as Convert<BorrowAsset, CollateralAsset>>::convert(&price_pair, borrow);
    let _ = <PricePair as Convert<CollateralAsset, BorrowAsset>>::convert(&price_pair, coll);
});
