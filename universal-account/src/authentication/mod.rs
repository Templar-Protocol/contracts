use near_sdk::{near, AccountIdRef};
use schemars::JsonSchema;

use crate::ExecutionParameters;

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
    #[error("Executor account ID mismatch")]
    ExecutorAccountIdMismatch,
    #[error("Block height mismatch")]
    BlockHeightMismatch,
    #[error("Key index mismatch")]
    KeyIndexMismatch,
    #[error("Nonce mismatch")]
    NonceMismatch,
    #[error("Origin unknown")]
    OriginUnknown,
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
        executor_account_id: &AccountIdRef,
        parameters: &ExecutionParameters,
        allowed_origin: impl FnOnce(Option<&str>) -> bool,
    ) -> Result<Self::Payload, ExecutionError> {
        let origin = self.origin();
        if !allowed_origin(origin) {
            return Err(ExecutionError::OriginUnknown);
        }

        let payload = self.payload();
        if payload.account_id != executor_account_id {
            return Err(ExecutionError::ExecutorAccountIdMismatch);
        }

        if payload.parameters.block_height != parameters.block_height {
            return Err(ExecutionError::BlockHeightMismatch);
        }

        if payload.parameters.index != parameters.index {
            return Err(ExecutionError::KeyIndexMismatch);
        }

        if payload.parameters.nonce != parameters.nonce {
            return Err(ExecutionError::NonceMismatch);
        }

        Ok(payload.payload)
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
