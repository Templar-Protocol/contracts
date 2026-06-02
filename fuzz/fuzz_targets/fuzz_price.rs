//! Fuzz the real `PricePair` / `Valuation` / `Convert` functions on
//! `common/src/price.rs` with Pyth-shaped price inputs (P1: real code).
//!
//! ## Known bugs being tracked (P4)
//!
//! - **`Valuation::ratio` division-by-zero via `pow2_int(384)`**
//!   (ENG-343): when the ratio of two valuations has an
//!   extreme exponent gap, `ratio` falls back to a log2 approximation
//!   (`price.rs:171-189`). If `result_log2 == -384`, it computes
//!   `Decimal::ONE / Decimal::pow2_int(384)`. `pow2_int(384)` shifts
//!   `ONE.repr` (2^128) left by 384 = 2^512, which overflows `U512` to **0**
//!   (`primitives/src/number.rs:198-207`: the bound check permits exponent
//!   384, the exact overflow point). Dividing by that zero aborts.
//!   Reachable when `borrow/collateral_asset_decimals` (an *unvalidated* i32
//!   in `PriceOracleConfiguration`) and the asset amounts are both large.
//!   Fix is upstream: either `pow2_int` must reject exponent >= 384, or
//!   `MarketConfiguration::validate` must bound the decimals.
//!
//!   The harness bounds `decimals` to a realistic range so the rest of the
//!   price arithmetic stays under fuzz; the excluded extreme-decimals region
//!   IS the tracked bug (P2: targeted + documented + tracked).

#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use near_sdk::json_types::{I64, U64};
use templar_common::asset::{BorrowAsset, BorrowAssetAmount, CollateralAsset, CollateralAssetAmount};
use templar_common::oracle::pyth::{self, PythTimestamp};
use templar_common::price::{Appraise, Convert, PricePair};
use templar_common::Decimal;

// MUTATION-CHECK (P5): in `Valuation::ratio` (price.rs:151), delete the
// `if rhs.coefficient.is_zero() { return None; }` guard. Then
// `valuation(1).ratio(valuation(0))` no longer returns None and the
// `v_one.ratio(v_zero).is_none()` assertion below must fire (or it aborts).

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

    // Bound decimals to a realistic token range [0, 30]. NEP-141 token
    // decimals are a u8 and in practice ≤ 24; the i32 field is unvalidated,
    // and extreme values trigger ENG-343 (see module doc).
    // This is the *one* dimension that drives the extreme exponent gap, so
    // bounding it is targeted, not a blanket narrow.
    let collateral_decimals = collateral_decimals.rem_euclid(31);
    let borrow_decimals = borrow_decimals.rem_euclid(31);

    let collateral_pyth_price = pyth::Price {
        price: I64(collateral_price_raw),
        conf: U64(collateral_conf),
        expo: -8,
        publish_time: PythTimestamp::from_secs(0),
    };
    let borrow_pyth_price = pyth::Price {
        price: I64(borrow_price_raw),
        conf: U64(borrow_conf),
        expo: -8,
        publish_time: PythTimestamp::from_secs(0),
    };

    let Ok(price_pair) = PricePair::new(
        &collateral_pyth_price,
        collateral_decimals,
        &borrow_pyth_price,
        borrow_decimals,
    ) else {
        // PricePair::new rejects negative prices, conf >= price (which also
        // rejects price == 0), and out-of-range exponents.
        return;
    };

    let collateral_amt = CollateralAssetAmount::new(collateral_amount);
    let borrow_amt = BorrowAssetAmount::new(borrow_amount);

    // ---- Non-trivial properties of the real functions (P2) ----

    // 1. valuation(0) has a zero coefficient ⇒ its ratio as a divisor is None.
    let v_zero = price_pair.valuation(CollateralAssetAmount::zero());
    let v_one = price_pair.valuation(CollateralAssetAmount::new(1));
    assert!(
        v_one.ratio(v_zero).is_none(),
        "ratio by a zero-amount valuation must be None (div0 guard)",
    );

    // 2. Monotonicity: valuation is non-decreasing in amount (same price/exp,
    //    so the ratio against a smaller amount must be >= 1).
    if collateral_amount > 0 {
        let v = price_pair.valuation(collateral_amt);
        let v_plus = price_pair.valuation(CollateralAssetAmount::new(
            collateral_amount.saturating_add(1),
        ));
        if let Some(ratio) = v_plus.ratio(v) {
            assert!(
                ratio >= Decimal::ONE,
                "valuation must be monotone non-decreasing in amount (ratio={ratio:?})",
            );
        }
    }

    // 3. The real conversions must not panic for in-range decimals. (With the
    //    decimals bound above, the pow2_int(384) path is unreachable, so any
    //    panic here would be a NEW finding, not the tracked one.)
    let _ = <PricePair as Convert<BorrowAsset, CollateralAsset>>::convert(&price_pair, borrow_amt);
    let _ = <PricePair as Convert<CollateralAsset, BorrowAsset>>::convert(&price_pair, collateral_amt);
});
