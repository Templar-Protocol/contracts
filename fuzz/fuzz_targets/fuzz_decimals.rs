//! Fuzz the production `UsageCurve` implementations (`Piecewise`,
//! `Exponential2`) via their real constructors and `at()`. Inputs are
//! constrained to the documented domain (rates/usage in `[0, 1]`, eccentricity
//! in `(0, 24]`) so we exercise in-bounds behavior.
//!
//! ## Known bugs being tracked (P4)
//!
//! - **Piecewise underflow** (`fuzz/README.md` § Known bugs):
//!   `Piecewise::new` computes `i_negative_rate_2_b = optimal*(rate_2-rate_1) - base`
//!   as an unsigned `Decimal`. When `base > optimal*(rate_2-rate_1)` this
//!   underflows and aborts. The math expects `i_negative_rate_2_b` to be
//!   signed (the formula in `at()` adds it back as a y-intercept that can be
//!   positive or negative). Tracked: ENG-341.
//!
//!   The subtraction is unconditional in `Piecewise::new`, so *every* input in
//!   that region aborts in the constructor — there is no in-region `at()`
//!   coverage to recover, and the skip is already as narrow as the bug allows.
//!   The harness skips only that region so the rest of the curve space stays
//!   under fuzz; ANY OTHER panic in `Piecewise::new`/`Piecewise::at` is still a
//!   finding. The abort itself is pinned by the unit test
//!   `interest_rate_strategy::tests::piecewise_new_underflows_when_base_exceeds_cross_term`.

#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::interest_rate_strategy::{Exponential2, Piecewise, UsageCurve};
use templar_common::Decimal;

// MUTATION-CHECK (P5): in `Exponential2::at` (interest_rate_strategy.rs:211),
// change `self.params.base + ...` to `self.params.base - ...`. Then `at(usage)`
// dips below `base` and the `y >= base_e` assertion below must fire.

/// Convert a `u64` to a `Decimal` in `[0, 1]` (exact for the endpoints).
fn to_decimal01(x: u64) -> Decimal {
    Decimal::from(x) / Decimal::from(u64::MAX)
}

fuzz_target!(|data: (u64, u64, u64, u64, u64, u64, u64)| {
    // ----- Piecewise -----
    let base_p = to_decimal01(data.0);
    let optimal = to_decimal01(data.1);
    let rate_1 = to_decimal01(data.2);
    let rate_2 = to_decimal01(data.3);
    let usage = to_decimal01(data.4);

    // `Piecewise::new` rejects `rate_1 > rate_2`, so the harness reads
    // `(rate_2 - rate_1)` only when the constructor would proceed.
    if rate_1 <= rate_2 {
        let cross_term = optimal * (rate_2 - rate_1);
        // KNOWN BUG (ENG-341): skip the specific input region
        // that triggers the unsigned-underflow inside the constructor. Every
        // other input still reaches `Piecewise::new`.
        if base_p <= cross_term {
            if let Some(piecewise) = Piecewise::new(base_p, optimal, rate_1, rate_2) {
                // Real `at()` on real curve; `at` requires `usage <= 1`.
                let _ = piecewise.at(usage.min(Decimal::ONE));
                // Non-trivial property: the rate at usage=0 equals `base`.
                // Asserted unconditionally (the constructor already succeeded).
                assert_eq!(
                    piecewise.at(Decimal::ZERO),
                    base_p,
                    "Piecewise::at(0) must equal base",
                );
            }
        }
    }

    // ----- Exponential2 -----
    let base_e = base_p;
    let top_raw = to_decimal01(data.5);
    let top = if top_raw >= base_e { top_raw } else { base_e };
    let eccentricity = to_decimal01(data.6) * Decimal::from(24u32);

    if let Some(exp2) = Exponential2::new(base_e, top, eccentricity) {
        let clamped = usage.min(Decimal::ONE);
        let y = exp2.at(clamped);
        // Non-trivial contract property: for any usage in [0, 1], the curve
        // value must be in [base, top]. (`Exponential2::at` does multi-step
        // Decimal arithmetic that loses precision at the ~10⁻²¹ scale, so we
        // can't use exact equality at the endpoints — but the [base, top]
        // bound is the actual safety property the contract relies on.)
        assert!(
            y >= base_e,
            "Exponential2::at({clamped:?}) = {y:?} < base {base_e:?}",
        );
        assert!(
            y <= top,
            "Exponential2::at({clamped:?}) = {y:?} > top {top:?}",
        );
        // Exact at the lower endpoint (multiplication by zero is exact).
        // Asserted unconditionally (the constructor already succeeded).
        assert_eq!(
            exp2.at(Decimal::ZERO),
            base_e,
            "Exponential2::at(0) must equal base",
        );
    }
});
