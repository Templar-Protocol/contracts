use near_sdk::{near, serde_json};
use schemars::JsonSchema;

use crate::PayloadExecutionParameters;

pub mod ed25519_raw;
pub mod eip712;
pub mod passkey;
mod payload;
pub use payload::*;
pub mod with_raw_string;

pub trait SignableMessage {
    type Key: Key<Self>
    where
        Self: Sized;
    type Signature: JsonSchema;
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct MessageWithSignature<M: SignableMessage> {
    pub message: M,
    pub signature: M::Signature,
}

pub struct MessageWithValidSignature<M: SignableMessage>(MessageWithSignature<M>);

#[derive(Debug, thiserror::Error, PartialEq, Eq, PartialOrd, Ord)]
pub enum CheckSignatureError {
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Signature verification error: {0}")]
    Other(Box<str>),
}

impl CheckSignatureError {
    pub fn other(e: impl std::error::Error) -> Self {
        Self::Other(e.to_string().into())
    }
}

pub trait Key<M: SignableMessage> {
    /// # Errors
    ///
    /// - If the signature is not valid.
    fn check_signature(&self, mws: &MessageWithSignature<M>) -> Result<(), CheckSignatureError>;

    /// # Errors
    ///
    /// - If [`Key::check_signature`] returns an error.
    fn verify_signature(
        &self,
        mws: MessageWithSignature<M>,
    ) -> Result<MessageWithValidSignature<M>, CheckSignatureError> {
        self.check_signature(&mws)
            .map(|()| MessageWithValidSignature(mws))
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExecutionError {
    #[error("Execution parameter `{field}` mismatch: expected `{expected}`, got `{actual}`")]
    Mismatch {
        field: &'static str,
        expected: Box<str>,
        actual: Box<str>,
    },
    #[error("Origin unknown")]
    OriginUnknown,
}

impl ExecutionError {
    pub fn mismatch(
        field: &'static str,
        expected: impl Into<Box<str>>,
        actual: impl Into<Box<str>>,
    ) -> Self {
        Self::Mismatch {
            field,
            expected: expected.into(),
            actual: actual.into(),
        }
    }
}

pub trait ExecutionContextProvider
where
    Self: Sized,
{
    type Payload;

    fn payload(self) -> Payload<Self::Payload>;
    fn origin(&self) -> Option<&str>;

    /// # Errors
    ///
    /// - If the executor account ID does not match.
    /// - If the execution parameters (nonce, key index) do not match.
    fn verify_execution(
        self,
        expected_parameters: &PayloadExecutionParameters,
        allowed_origin: impl FnOnce(Option<&str>) -> bool,
    ) -> Result<Self::Payload, ExecutionError> {
        let origin = self.origin();
        if !allowed_origin(origin) {
            return Err(ExecutionError::OriginUnknown);
        }

        let payload = self.payload();

        macro_rules! check_field {
            ($parameters:ident,$field:ident.$($string_repr:tt)+) => {
                if $parameters.$field != expected_parameters.$field {
                    return Err(ExecutionError::Mismatch {
                        field: stringify!($field).into(),
                        expected: expected_parameters.$field.$($string_repr)+.into(),
                        actual: $parameters.$field.$($string_repr)+.into(),
                    });
                }
            };
        }

        let parameters = payload.parameters();

        check_field!(parameters, block_height.0.to_string());
        check_field!(
            parameters,
            chain_id.map_or("<none>".to_string(), |c| c.0.to_string())
        );
        check_field!(parameters, index.0.to_string());
        check_field!(parameters, name.clone().unwrap_or("<none>".to_string()));
        check_field!(parameters, nonce.0.to_string());
        check_field!(
            parameters,
            salt.as_ref().map_or("<none>".to_string(), |s| {
                #[allow(clippy::unwrap_used, reason = "Infallible")]
                serde_json::to_string(&s).unwrap()
            })
        );
        check_field!(parameters, verifying_contract.as_str());
        check_field!(parameters, version.clone().unwrap_or("<none>".to_string()));

        Ok(payload.payload())
    }
}

pub trait HashForSigning {
    const MAGIC_NUMBER: &'static [u8];

    fn content_bytes(&self) -> Vec<u8>;

    fn preimage_for_signing(&self) -> Vec<u8> {
        [Self::MAGIC_NUMBER.to_vec(), self.content_bytes()].concat()
    }

    fn hash_for_signing(&self) -> [u8; 32] {
        near_sdk::env::sha256_array(&self.preimage_for_signing())
    }
}
