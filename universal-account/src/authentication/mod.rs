use std::fmt::Debug;

use near_sdk::{
    near,
    serde::{de::DeserializeOwned, Serialize},
    serde_json,
};
use schemars::JsonSchema;

use crate::PayloadExecutionParameters;

pub mod ed25519;
pub mod eip712;
pub mod passkey;
mod payload;
pub use payload::*;
pub mod with_raw_string;

macro_rules! verify_key {
    ($inner: ty) => {
        #[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
        #[::near_sdk::near(serializers = [borsh, json])]
        pub struct VerifyKey(pub $inner);

        impl From<$inner> for VerifyKey {
            fn from(value: $inner) -> Self {
                Self(value)
            }
        }

        impl AsRef<$inner> for VerifyKey {
            fn as_ref(&self) -> &$inner {
                &self.0
            }
        }

        impl ::std::ops::Deref for VerifyKey {
            type Target = $inner;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl ::std::fmt::Display for VerifyKey {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}
use verify_key;

pub trait SignableMessage {
    type Key: Key<Self>
    where
        Self: Sized;
    type Signature: JsonSchema + Serialize + DeserializeOwned + Clone + Debug;
    type Auxiliary: JsonSchema + Serialize + DeserializeOwned + Clone + Debug;
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct MessageWithSignature<M: SignableMessage> {
    pub message: M,
    pub signature: M::Signature,
    #[serde(flatten)]
    pub auxiliary: M::Auxiliary,
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
        near_sdk::env::sha256_array(self.preimage_for_signing())
    }
}

#[cfg(kani)]
mod kani_proofs {
    use near_sdk::{json_types::U64, AccountId};

    use crate::{authentication::ed25519::raw, encoding, PayloadExecutionParameters};

    use super::*;

    // These proofs start from a message whose signature has already been accepted.
    // They prove execution-parameter and payload plumbing, not cryptographic binding.
    fn account_id() -> AccountId {
        "account.near".parse().unwrap()
    }

    fn execution_parameters() -> PayloadExecutionParameters {
        PayloadExecutionParameters::builder_empty()
            .block_height(7_u64)
            .index(3_u64)
            .nonce(11_u64)
            .verifying_contract(account_id())
            .build()
    }

    fn valid_raw_message(payload: u8) -> MessageWithValidSignature<raw::Message<u8>> {
        let message = raw::Message::from_parsed(Payload::new(execution_parameters(), payload));

        MessageWithValidSignature(MessageWithSignature {
            message,
            signature: encoding::ed25519::Signature([0u8; 64]),
            auxiliary: (),
        })
    }

    #[kani::proof]
    fn valid_message_execution_returns_exact_signed_payload() {
        let signed_payload = kani::any::<u8>();

        let returned = valid_raw_message(signed_payload)
            .verify_execution(&execution_parameters(), |_| true)
            .unwrap();

        assert_eq!(returned, signed_payload);
    }

    #[kani::proof]
    fn valid_message_execution_rejects_parameter_mismatch_before_payload_return() {
        let signed_payload = kani::any::<u8>();
        let mut expected_parameters = execution_parameters();
        expected_parameters.nonce = U64(expected_parameters.nonce.0 + 1);

        let result =
            valid_raw_message(signed_payload).verify_execution(&expected_parameters, |_| true);

        assert!(matches!(
            result,
            Err(ExecutionError::Mismatch { field: "nonce", .. })
        ));
    }
}
