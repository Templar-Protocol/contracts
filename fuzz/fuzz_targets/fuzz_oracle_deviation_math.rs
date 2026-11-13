//! Hold the four circuit-breaker rules' `should_trip` to the exact arbitrary-precision
//! reference in `templar_fuzz::oracle` over their full input domain: any positive `i64`
//! mantissa (accepted history and baselines only ever hold validated, strictly positive
//! prices), any `i32` exponent, and any `Decimal` threshold. Where a comparison fits the
//! kernel's 512-bit word the decisions must agree exactly; where it does not, the rule must
//! trip (fail-closed).
//!
//! MUTATION-CHECK (P5): in `compare_scaled_change`
//! (contract/proxy-oracle/kernel/src/proxy/circuit_breaker/math.rs), change the strict
//! `left > right` to `left >= right`. A move landing exactly on a stepwise, windowed or
//! cumulative threshold then trips, and the reference comparison must fire.

#![no_main]
#![allow(
    clippy::expect_used,
    reason = "panics on invariant violation are the intended libFuzzer crash signal"
)]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use templar_fuzz::oracle::{self, input::RuleInput};
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::{
    proxy::circuit_breaker::{CircuitBreakerRule, Observation, RingBuffer},
    Price,
};

const MAX_HISTORY_LEN: usize = 64;

/// Exponent relative to the scenario's base (dense, realistic gaps) or absolute (extreme gaps).
#[derive(Arbitrary, Debug, Clone, Copy)]
enum ExponentInput {
    Near(i8),
    Absolute(i32),
}

#[derive(Arbitrary, Debug, Clone, Copy)]
struct PriceInput {
    mantissa: u64,
    exponent: ExponentInput,
}

impl PriceInput {
    fn price(self, base_exponent: i32) -> Price {
        Price {
            price: i64::try_from(self.mantissa >> 1).map_or(1, |mantissa| mantissa.max(1)),
            conf: 0,
            expo: match self.exponent {
                ExponentInput::Near(offset) => base_exponent.saturating_add(offset.into()),
                ExponentInput::Absolute(exponent) => exponent,
            },
            publish_time_ns: Nanoseconds::zero(),
        }
    }
}

#[derive(Arbitrary, Debug)]
struct Input {
    base_exponent: i32,
    rule: RuleInput<PriceInput>,
    history: Vec<PriceInput>,
}

fuzz_target!(|input: Input| {
    let breaker = input
        .rule
        .breaker(|baseline| baseline.price(input.base_exponent));
    let prices = input
        .history
        .iter()
        .take(MAX_HISTORY_LEN)
        .map(|price| price.price(input.base_exponent))
        .collect::<Vec<_>>();
    let mut history = RingBuffer::new(u32::try_from(prices.len()).expect("history is capped"));
    for price in &prices {
        history.push(Observation {
            price: *price,
            observed_at_ns: Nanoseconds::zero(),
        });
    }

    assert_eq!(
        breaker.should_trip(&history),
        oracle::should_trip(&breaker, &prices),
        "{breaker:?} on {prices:?}"
    );
});
