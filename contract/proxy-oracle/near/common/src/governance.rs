use near_sdk::near;
use templar_common::{
    gen_ext_governance, governance::Validatable, oracle::pyth::PriceIdentifier, Nanoseconds,
};
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{CircuitBreaker, CircuitBreakerSetConfig},
    Proxy,
};

use crate::input::Source;

pub const MAX_CIRCUIT_BREAKER_HISTORY_LEN: u32 = 32;
pub const MAX_CIRCUIT_BREAKERS_PER_PROXY: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum CircuitBreakerUpdate {
    /// Controls whether this breaker blocks the feed when tripped.
    ///
    /// Non-enforced breakers still evaluate and can become tripped; they just do not make the
    /// containing set block price resolution.
    SetEnforced { is_enforced: bool },
    /// Set the absolute timestamp after which the breaker can trip.
    ///
    /// `timestamp_ns = 0` is the canonical immediately-armed value. This does not change
    /// enforcement.
    SetArmedAfter { timestamp_ns: Nanoseconds },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Operation {
    SetProxy {
        id: PriceIdentifier,
        proxy: Option<Proxy<Source>>,
    },
    SetActionTtl {
        new_ttl: Nanoseconds,
    },
    /// Update shared sampling/history configuration for a proxy's circuit breaker set.
    ///
    /// `history_len = 0` is allowed and means no observations are retained. This is useful for
    /// coherent no-op configurations, but installed breakers that require prior observations will
    /// not trip until history capacity is increased and enough samples have accumulated.
    ConfigureCircuitBreakers {
        id: PriceIdentifier,
        config: CircuitBreakerSetConfig,
    },
    SetCircuitBreakerManualTrip {
        id: PriceIdentifier,
        is_manually_tripped: bool,
    },
    AddCircuitBreaker {
        id: PriceIdentifier,
        /// Expected next breaker ID within the set.
        ///
        /// The contract rejects the operation unless this matches the set's current `next_id`,
        /// keeping breaker IDs explicit while preserving monotonic assignment.
        breaker_id: u32,
        /// Breaker rule to add to the set.
        ///
        /// Adding a breaker does not implicitly resize retained history. If the set keeps too few
        /// observations for the rule, the breaker remains armed/enforced but cannot trip until
        /// enough history can be retained and has accumulated.
        breaker: CircuitBreaker,
    },
    RemoveCircuitBreaker {
        id: PriceIdentifier,
        breaker_id: u32,
    },
    UpdateCircuitBreaker {
        id: PriceIdentifier,
        breaker_id: u32,
        update: CircuitBreakerUpdate,
    },
}

impl Validatable for Operation {
    type OnCreateError = ValidationError;
    type OnExecuteError = ValidationError;

    fn on_create(&self) -> Result<(), Self::OnCreateError> {
        match self {
            Operation::SetProxy {
                proxy: Some(proxy), ..
            } if proxy.sources().len() == 0 => Err(ValidationError::EmptyProxyDefinition),
            Operation::ConfigureCircuitBreakers { config, .. }
                if config.history_len > MAX_CIRCUIT_BREAKER_HISTORY_LEN =>
            {
                Err(ValidationError::CircuitBreakerHistoryTooLong {
                    maximum: MAX_CIRCUIT_BREAKER_HISTORY_LEN,
                    actual: config.history_len,
                })
            }
            _ => Ok(()),
        }
    }

    fn on_execute(&self) -> Result<(), Self::OnExecuteError> {
        self.on_create()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("Empty proxy definition is not allowed")]
    EmptyProxyDefinition,
    #[error("Circuit breaker history length is too long: maximum {maximum}, got {actual}")]
    CircuitBreakerHistoryTooLong { maximum: u32, actual: u32 },
}

gen_ext_governance!(ext_proxy_governance, ProxyGovernanceInterface, Operation);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::OracleRequest;
    use rstest::rstest;
    use templar_proxy_oracle_kernel::proxy::{Aggregator, FreshnessFilter};

    fn invalid_operation() -> Operation {
        Operation::SetProxy {
            id: PriceIdentifier([0xaa; 32]),
            proxy: Some(Proxy::new(
                Aggregator::median_low([]),
                FreshnessFilter::empty(),
            )),
        }
    }

    fn valid_operation() -> Operation {
        Operation::SetProxy {
            id: PriceIdentifier([0xff; 32]),
            proxy: Some(Proxy::new(
                Aggregator::median_low([OracleRequest::pyth(
                    "pyth-oracle.near".parse().unwrap(),
                    PriceIdentifier([0xdd; 32]),
                )
                .into()]),
                FreshnessFilter::empty(),
            )),
        }
    }

    #[rstest]
    #[case::valid(valid_operation())]
    #[should_panic = "EmptyProxyDefinition"]
    #[case::invalid(invalid_operation())]
    fn on_create(#[case] operation: Operation) {
        operation.on_create().unwrap();
    }

    #[rstest]
    #[case::valid(valid_operation())]
    #[should_panic = "EmptyProxyDefinition"]
    #[case::invalid(invalid_operation())]
    fn on_execute(#[case] operation: Operation) {
        operation.on_execute().unwrap();
    }

    #[test]
    fn configure_circuit_breakers_rejects_excessive_history_len() {
        let operation = Operation::ConfigureCircuitBreakers {
            id: PriceIdentifier([0xaa; 32]),
            config: CircuitBreakerSetConfig {
                sample_interval_ns: Nanoseconds::zero(),
                history_len: MAX_CIRCUIT_BREAKER_HISTORY_LEN + 1,
            },
        };

        assert_eq!(
            operation.on_create(),
            Err(ValidationError::CircuitBreakerHistoryTooLong {
                maximum: MAX_CIRCUIT_BREAKER_HISTORY_LEN,
                actual: MAX_CIRCUIT_BREAKER_HISTORY_LEN + 1,
            })
        );
        assert_eq!(operation.on_execute(), operation.on_create());
    }
}
