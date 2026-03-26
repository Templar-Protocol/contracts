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
            } if proxy.entries.is_empty() => Err(ValidationError::EmptyProxyDefinition),
            _ => Ok(()),
        }
    }

    fn on_execute(&self) -> Result<(), Self::OnExecuteError> {
        self.on_create()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Empty proxy definition is not allowed")]
    EmptyProxyDefinition,
}

gen_ext_governance!(ext_proxy_governance, ProxyGovernanceInterface, Operation);
