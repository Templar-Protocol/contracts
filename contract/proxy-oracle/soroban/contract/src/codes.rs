//! Numeric codes for events and structured errors. Kept centralized so any
//! tweak shows up in one place rather than scattered across publish paths.

use crate::{AGGREGATION_FAILED_CODE, STORAGE_FAILED_CODE};
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{CircuitBreaker, CircuitBreakerError, PriceBlockedReason},
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
        CircuitBreaker::CumulativeChange(_) => 4,
    }
}

pub fn resolve_error_code(error: ResolveError) -> u32 {
    match error {
        ResolveError::Aggregation(_) => AGGREGATION_FAILED_CODE,
        ResolveError::CircuitBreaker(_) => STORAGE_FAILED_CODE,
    }
}

pub fn breaker_error(error: CircuitBreakerError) -> ContractError {
    match error {
        CircuitBreakerError::TooManyBreakers => ContractError::TooManyBreakers,
        CircuitBreakerError::BreakerNotFound { .. }
        | CircuitBreakerError::UnexpectedBreakerId { .. }
        | CircuitBreakerError::InvalidPrice
        | CircuitBreakerError::InvalidConfiguration => ContractError::BreakerError,
    }
}

#[cfg(test)]
mod tests {
    use templar_proxy_oracle_kernel::proxy::{circuit_breaker::CircuitBreakerError, ResolveError};

    use super::{resolve_error_code, AGGREGATION_FAILED_CODE, STORAGE_FAILED_CODE};

    #[test]
    fn unexpected_breaker_failures_map_to_storage() {
        assert_eq!(
            resolve_error_code(ResolveError::CircuitBreaker(
                CircuitBreakerError::InvalidPrice
            )),
            STORAGE_FAILED_CODE
        );
        assert_ne!(STORAGE_FAILED_CODE, AGGREGATION_FAILED_CODE);
    }
}
