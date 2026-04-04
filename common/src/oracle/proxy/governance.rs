use near_sdk::near;

use crate::{
    gen_ext_governance, governance::Validatable, oracle::pyth::PriceIdentifier, time::Nanoseconds,
};

use super::Proxy;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Operation {
    SetProxy {
        id: PriceIdentifier,
        proxy: Option<Proxy>,
    },
    SetActionTtl {
        new_ttl: Nanoseconds,
    },
}

impl Validatable for Operation {
    type OnCreateError = ValidationError;
    type OnExecuteError = ValidationError;

    fn on_create(&self) -> Result<(), Self::OnCreateError> {
        match self {
            Operation::SetProxy {
                proxy: Some(proxy), ..
            } if proxy.sources().is_empty() => Err(ValidationError::EmptyProxyDefinition),
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
    use crate::oracle::{
        proxy::{Aggregator, FreshnessFilter},
        OracleRequest,
    };
    use rstest::rstest;

    fn invalid_operation() -> Operation {
        Operation::SetProxy {
            id: PriceIdentifier([0xaa; 32]),
            proxy: Some(Proxy::new(
                Aggregator::median_low([]),
                FreshnessFilter::default(),
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
                FreshnessFilter::default(),
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
