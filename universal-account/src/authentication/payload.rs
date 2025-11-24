use alloy::sol;
use near_sdk::{
    near,
    serde::{de::DeserializeOwned, Serialize},
    serde_json, AccountId,
};

use crate::{ExecutionParameters, SolExecutionParameters};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json])]
#[serde(deny_unknown_fields)]
pub struct Payload<T> {
    pub parameters: ExecutionParameters,
    pub account_id: AccountId,
    pub payload: T,
}

sol! {
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct SolPayload {
        SolExecutionParameters parameters;
        string account_id;
        bytes payload;
    }
}

#[derive(thiserror::Error, Debug)]
pub enum SolPayloadParseError {
    #[error(transparent)]
    AccountIdParse(#[from] near_account_id::ParseAccountError),
    #[error(transparent)]
    JsonParse(#[from] serde_json::Error),
}

impl<T: DeserializeOwned> TryFrom<SolPayload> for Payload<T> {
    type Error = SolPayloadParseError;

    fn try_from(value: SolPayload) -> Result<Self, Self::Error> {
        Ok(Self {
            parameters: value.parameters.into(),
            account_id: value.account_id.parse()?,
            payload: serde_json::from_slice(&value.payload)?,
        })
    }
}

impl<T: Serialize> TryFrom<&Payload<T>> for SolPayload {
    type Error = serde_json::Error;

    fn try_from(value: &Payload<T>) -> Result<Self, Self::Error> {
        Ok(Self {
            parameters: value.parameters.into(),
            account_id: value.account_id.to_string(),
            payload: serde_json::to_vec(&value.payload)?.into(),
        })
    }
}

impl<T: Serialize> TryFrom<Payload<T>> for SolPayload {
    type Error = serde_json::Error;

    fn try_from(value: Payload<T>) -> Result<Self, Self::Error> {
        Ok(Self {
            parameters: value.parameters.into(),
            account_id: Box::<str>::from(value.account_id).into(),
            payload: serde_json::to_vec(&value.payload)?.into(),
        })
    }
}
