use near_sdk::near;
use templar_common::{
    gen_ext_governance, governance::Validatable, oracle::pyth::PriceIdentifier, Nanoseconds,
};
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{CircuitBreaker, CircuitBreakerSetConfig},
    Proxy,
};

use crate::input::Source;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum CircuitBreakerStatusUpdate {
    Enable,
    Disable,
    Arm,
    Mute { until_ns: Nanoseconds },
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
        /// Breaker rule to add to the set.
        ///
        /// Adding a breaker does not implicitly resize retained history. If the set keeps too few
        /// observations for the rule, the breaker remains armed/enabled but cannot trip until
        /// enough history can be retained and has accumulated.
        breaker: CircuitBreaker,
    },
    RemoveCircuitBreaker {
        id: PriceIdentifier,
        breaker_id: u32,
    },
    SetCircuitBreakerStatus {
        id: PriceIdentifier,
        breaker_id: u32,
        status: CircuitBreakerStatusUpdate,
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
}
