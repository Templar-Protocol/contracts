use alloy::sol;
use near_sdk::near;

use crate::{KeyParameters, PayloadExecutionParameters};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json])]
#[serde(tag = "version")]
pub enum Payload<T> {
    #[serde(rename = "1")]
    V1(PayloadV1<T>),
    #[serde(untagged)]
    V0(PayloadV0<T>),
}

impl<T> Payload<T> {
    pub fn new(parameters: PayloadExecutionParameters, payload: T) -> Self {
        Self::V1(PayloadV1 {
            parameters,
            payload,
        })
    }

    pub fn payload_ref(&self) -> &T {
        match self {
            Self::V1(v1) => &v1.payload,
            Self::V0(v0) => &v0.payload,
        }
    }

    pub fn payload_mut(&mut self) -> &mut T {
        match self {
            Self::V1(v1) => &mut v1.payload,
            Self::V0(v0) => &mut v0.payload,
        }
    }

    pub fn payload(self) -> T {
        match self {
            Self::V1(v1) => v1.payload,
            Self::V0(v0) => v0.payload,
        }
    }

    pub fn parameters(&self) -> PayloadExecutionParameters {
        match self {
            Self::V1(v1) => v1.parameters.clone(),
            Self::V0(v0) => PayloadExecutionParameters::builder_empty()
                .verifying_contract(v0.account_id.clone())
                .with_key_parameters(v0.parameters)
                .build(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json])]
pub struct PayloadV0<T> {
    pub account_id: near_sdk::AccountId,
    pub parameters: KeyParameters,
    pub payload: T,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json])]
pub struct PayloadV1<T> {
    pub parameters: PayloadExecutionParameters,
    pub payload: T,
}

sol! {
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct SolBytes {
        bytes inner;
    }
}
