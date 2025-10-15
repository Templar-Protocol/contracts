use near_sdk::AccountIdRef;

use crate::ExecutionParameters;

pub mod passkey;

#[derive(Debug, thiserror::Error)]
#[error("Invalid signature")]
pub struct InvalidSignatureError;

pub trait Key<M> {
    type Validated;

    /// # Errors
    ///
    /// - If checking the signature fails
    fn verify(&self, message: M) -> Result<Self::Validated, InvalidSignatureError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Executor account ID mismatch")]
    ExecutorAccountIdMismatch,
    #[error("Key index mismatch")]
    KeyIndexMismatch,
    #[error("Nonce mismatch")]
    NonceMismatch,
    #[error("Origin unknown")]
    OriginUnknown,
}

pub trait ExecutionContextProvider {
    type Payload;

    fn account_id(&self) -> &AccountIdRef;
    fn parameters(&self) -> &ExecutionParameters;
    fn payload_unchecked(&self) -> &Self::Payload;
    fn origin(&self) -> Option<&str>;

    /// # Errors
    ///
    /// - If the executor account ID does not match.
    /// - If the execution parameters (nonce, key index) do not match.
    fn verify(
        &self,
        executor_account_id: &AccountIdRef,
        parameters: &ExecutionParameters,
        allowed_origin: impl FnOnce(Option<&str>) -> bool,
    ) -> Result<&Self::Payload, ExecutionError> {
        if self.account_id() != executor_account_id {
            return Err(ExecutionError::ExecutorAccountIdMismatch);
        }

        let p = self.parameters();
        if p.index != parameters.index {
            return Err(ExecutionError::KeyIndexMismatch);
        }

        if p.nonce != parameters.nonce {
            return Err(ExecutionError::NonceMismatch);
        }

        if !allowed_origin(self.origin()) {
            return Err(ExecutionError::OriginUnknown);
        }

        Ok(self.payload_unchecked())
    }
}

pub trait MagicNumber {
    const MAGIC_NUMBER: &'static [u8];
}
