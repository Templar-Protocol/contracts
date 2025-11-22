use near_sdk::AccountIdRef;

use crate::ExecutionParameters;

pub mod ed25519_raw;
pub mod eip712;
pub mod passkey;
mod payload;
pub use payload::*;
pub mod with_raw_string;

#[derive(Debug, thiserror::Error, PartialEq, Eq, PartialOrd, Ord)]
#[error("Invalid signature")]
pub struct InvalidSignatureError;

pub trait Key<M> {
    type Verified;

    /// # Errors
    ///
    /// - If checking the signature fails
    fn verify_signature(&self, message: M) -> Result<Self::Verified, InvalidSignatureError>;
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
