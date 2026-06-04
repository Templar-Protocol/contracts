//! Numeric codes for events and structured errors. Kept centralized so any
//! tweak shows up in one place rather than scattered across publish paths.

use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{
        AcceptedHistorySource, CircuitBreaker, CircuitBreakerError, PriceBlockedReason,
    },
    ResolveError,
};
use templar_proxy_oracle_soroban_common::ContractError;

pub fn blocked_reason_code(reason: PriceBlockedReason) -> u32 {
    match reason {
        PriceBlockedReason::ManuallyTripped => 1,
        PriceBlockedReason::BreakerTripped { .. } => 2,
    }
}

pub fn breaker_kind_code(breaker: &CircuitBreaker) -> u32 {
    match breaker {
        CircuitBreaker::StepwiseChange(_) => 1,
        CircuitBreaker::MonotonicRun(_) => 2,
        CircuitBreaker::WindowedChangeDelta(_) => 3,
    }
}

pub fn accepted_history_source_code(source: AcceptedHistorySource) -> u32 {
    match source {
        AcceptedHistorySource::Empty => 0,
        AcceptedHistorySource::Observed => 1,
    }
}

pub fn resolve_error_code(error: ResolveError) -> u32 {
    match error {
        ResolveError::Aggregation(_) => 1,
        ResolveError::CircuitBreaker(_) => 2,
    }
}

pub fn breaker_error(error: CircuitBreakerError) -> ContractError {
    match error {
        CircuitBreakerError::TooManyBreakers => ContractError::TooManyBreakers,
        CircuitBreakerError::BreakerNotFound { .. }
        | CircuitBreakerError::UnexpectedBreakerId { .. }
        | CircuitBreakerError::InvalidPrice => ContractError::BreakerError,
    }
}
