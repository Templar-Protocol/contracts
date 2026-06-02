//! Fuzz the real `InterestRateStrategy` enum + `Linear` usage curve and their
//! `UsageCurve::at` dispatch (P1: real production code).
//!
//! Scope (P6): this target owns the **enum-level** behaviour — `linear`/`zero`
//! constructors and `Deref`-based dispatch to `UsageCurve::at`. The internal
//! arithmetic of the `Piecewise` and `Exponential2` curves is the scope of
//! `fuzz_decimals`; we don't re-fuzz it here.
//!
//! The previous version of this target was a toy reimplementation — it computed
//! its own "simple/compound interest", "utilization", and "exchange rate"
//! formulas with fabricated constants (`base_rate = 200`, `slope = 1000`, …)
//! and asserted properties of that hand-rolled math, testing no contract code
//! (P1 violation).
//!
//! MUTATION-CHECK (P5): in `Linear::at` (interest_rate_strategy.rs:82), change
//! `usage_ratio * (self.top - self.base) + self.base` to `... - self.base`.
//! Then `at(0) == base` and the `at(usage) >= base` lower-bound assertions
//! below must fire.

#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::interest_rate_strategy::InterestRateStrategy;
use templar_common::Decimal;

/// Map a `u64` to a `Decimal` in `[0, 1]` (exact at the endpoints).
fn unit(x: u64) -> Decimal {
    Decimal::from(x) / Decimal::from(u64::MAX)
}

fuzz_target!(|data: (u64, u64, u64, u64)| {
    let (base_raw, top_raw, usage_raw, usage2_raw) = data;
    let base = unit(base_raw);
    let top = unit(top_raw);
    let usage = unit(usage_raw).min(Decimal::ONE);
    let usage2 = unit(usage2_raw).min(Decimal::ONE);

    // Constructor contract: `linear` succeeds iff base <= top (mirrors
    // `Linear::new`). This is a real property the constructor enforces.
    let built = InterestRateStrategy::linear(base, top);
    assert_eq!(
        built.is_some(),
        base <= top,
        "InterestRateStrategy::linear must succeed exactly when base <= top",
    );

    if let Some(strategy) = built {
        // at(0) == base exactly (Linear is exact: 0*(top-base)+base).
        assert_eq!(
            strategy.at(Decimal::ZERO),
            base,
            "linear.at(0) must equal base",
        );
        // at(1) == top exactly (1*(top-base)+base == top).
        assert_eq!(
            strategy.at(Decimal::ONE),
            top,
            "linear.at(1) must equal top",
        );

        // For any usage in [0, 1], the rate stays within [base, top]
        // (Linear arithmetic is exact, so these are hard bounds, not ≈).
        let y = strategy.at(usage);
        assert!(y >= base, "linear.at({usage:?}) = {y:?} < base {base:?}");
        assert!(y <= top, "linear.at({usage:?}) = {y:?} > top {top:?}");

        // Monotonic non-decreasing in usage (a buggy slope sign flips this).
        let (lo, hi) = if usage <= usage2 {
            (usage, usage2)
        } else {
            (usage2, usage)
        };
        assert!(
            strategy.at(lo) <= strategy.at(hi),
            "linear.at must be monotone non-decreasing: at({lo:?})={:?} > at({hi:?})={:?}",
            strategy.at(lo),
            strategy.at(hi),
        );
    }

    // The zero strategy yields a flat-zero rate everywhere.
    let zero = InterestRateStrategy::zero();
    assert_eq!(
        zero.at(usage),
        Decimal::ZERO,
        "zero strategy must return 0 for any usage",
    );
});
