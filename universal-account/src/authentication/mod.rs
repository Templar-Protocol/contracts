use near_sdk::AccountIdRef;

use crate::ExecutionParameters;

pub mod passkey;

#[derive(Debug, thiserror::Error)]
pub enum VerificationError {
    #[error("Invalid signature")]
    InvalidSignature,
    #[error(transparent)]
    Execution(#[from] ExecutionError),
}

pub trait Key<M: ExecutionContextProvider> {
    type Signature;

    fn is_signature_valid(&self, message: &M) -> bool;

    /// # Errors
    ///
    /// - If checking the signature fails
    fn check<'a>(
        &self,
        message: &'a M,
        executor_account_id: &AccountIdRef,
        parameters: &mut ExecutionParameters,
    ) -> Result<&'a M::Payload, VerificationError> {
        if !self.is_signature_valid(message) {
            return Err(VerificationError::InvalidSignature);
        }

        Ok(message.verify_and_increment_nonce(executor_account_id, parameters)?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Executor account ID mismatch")]
    ExecutorAccountIdMismatch,
    #[error("Key index mismatch")]
    KeyIndexMismatch,
    #[error("Nonce mismatch")]
    NonceMismatch,
}

pub trait ExecutionContextProvider {
    type Payload;
    type Signature;

    fn account_id(&self) -> &AccountIdRef;
    fn parameters(&self) -> &ExecutionParameters;
    fn payload_prehash(&self) -> Vec<u8>;
    fn signature(&self) -> &Self::Signature;
    fn payload_unchecked(&self) -> &Self::Payload;

    /// # Errors
    ///
    /// - If the executor account ID does not match.
    /// - If the execution parameters (nonce, key index) do not match.
    fn verify_and_increment_nonce(
        &self,
        executor_account_id: &AccountIdRef,
        parameters: &mut ExecutionParameters,
    ) -> Result<&Self::Payload, ExecutionError> {
        if self.account_id() != executor_account_id {
            return Err(ExecutionError::ExecutorAccountIdMismatch);
        }

        let p = self.parameters();
        if p.index != parameters.index {
            return Err(ExecutionError::KeyIndexMismatch);
        }

        if p.nonce.0 != parameters.nonce.0 + 1 {
            return Err(ExecutionError::NonceMismatch);
        }

        parameters.nonce.0 += 1;

        Ok(self.payload_unchecked())
    }
}
