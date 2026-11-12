//! Structured `Arbitrary` inputs shared by the oracle fuzz targets.

use arbitrary::Arbitrary;
use primitive_types::U512;
use templar_primitives::{number::FRACTIONAL_BITS, Decimal};
use templar_proxy_oracle_kernel::{
    proxy::circuit_breaker::{
        CircuitBreaker, CumulativeChange, MonotonicRun, StepwiseChange, WindowedChangeDelta,
    },
    Price,
};

/// A `Decimal` threshold, weighted toward the configurable range `(0, 1]` while still reaching
/// zero, values above one, and every representation.
#[derive(Arbitrary, Debug, Clone, Copy)]
pub enum ThresholdInput {
    BasisPoints(u16),
    Fraction(u128),
    One,
    Raw([u64; 8]),
}

impl ThresholdInput {
    pub fn decimal(self) -> Decimal {
        match self {
            Self::BasisPoints(basis_points) => Decimal::from_repr(
                ((U512::from(basis_points) << FRACTIONAL_BITS) / U512::from(10_000_u16)).0,
            ),
            Self::Fraction(fraction) => Decimal::from_repr(U512::from(fraction).0),
            Self::One => Decimal::ONE,
            Self::Raw(repr) => Decimal::from_repr(repr),
        }
    }
}

/// A rule's count parameter: usually small enough to pass configuration, sometimes any `u32`.
#[derive(Arbitrary, Debug, Clone, Copy)]
pub enum CountInput {
    Small(u8),
    Full(u32),
}

impl CountInput {
    pub fn get(self) -> u32 {
        match self {
            Self::Small(count) => count.into(),
            Self::Full(count) => count,
        }
    }
}

/// A circuit-breaker rule whose `CumulativeChange` baseline each target builds from its own
/// price input.
#[derive(Arbitrary, Debug, Clone)]
pub enum RuleInput<Baseline> {
    StepwiseChange {
        threshold: ThresholdInput,
    },
    MonotonicRun {
        max_streak: CountInput,
        threshold: ThresholdInput,
    },
    WindowedChangeDelta {
        window_len: CountInput,
        lookback_windows: CountInput,
        threshold: ThresholdInput,
    },
    CumulativeChange {
        baseline: Baseline,
        threshold: ThresholdInput,
    },
}

impl<Baseline> RuleInput<Baseline> {
    pub fn breaker(&self, baseline: impl FnOnce(&Baseline) -> Price) -> CircuitBreaker {
        match self {
            Self::StepwiseChange { threshold } => CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: threshold.decimal(),
            }),
            Self::MonotonicRun {
                max_streak,
                threshold,
            } => CircuitBreaker::MonotonicRun(MonotonicRun {
                max_streak: max_streak.get(),
                min_relative_step_change: threshold.decimal(),
            }),
            Self::WindowedChangeDelta {
                window_len,
                lookback_windows,
                threshold,
            } => CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta {
                window_len: window_len.get(),
                lookback_windows: lookback_windows.get(),
                max_relative_mean_change: threshold.decimal(),
            }),
            Self::CumulativeChange {
                baseline: input,
                threshold,
            } => CircuitBreaker::CumulativeChange(CumulativeChange {
                baseline: baseline(input),
                max_relative_change: threshold.decimal(),
            }),
        }
    }
}
