use near_sdk::AccountIdRef;

use crate::{Execute, ExecutionParameters};

pub mod passkey;

pub trait Key<M: ExecutionContextProvider> {
    type Signature;
    type Error: ToString;

    /// # Errors
    ///
    /// - If checking the signature fails
    fn verify_and_execute(
        &self,
        message: &M,
    ) -> Result<<M::Payload as Execute>::Output, Self::Error>;
}

#[derive(Debug, thiserror::Error)]
pub enum VerificationError {
    #[error("Executor account ID mismatch")]
    ExecutorAccountIdMismatch,
    #[error("Key index mismatch")]
    KeyIndexMismatch,
    #[error("Nonce mismatch")]
    NonceMismatch,
}

pub trait ExecutionContextProvider {
    type Payload: Execute;
    type Signature;

    fn account_id(&self) -> &AccountIdRef;
    fn parameters(&self) -> &ExecutionParameters;
    fn payload_prehash(&self) -> Vec<u8>;
    fn signature(&self) -> &Self::Signature;
    fn payload(&self) -> &Self::Payload;

    fn verify_and_increment_nonce(
        &self,
        executor_account_id: &AccountIdRef,
        parameters: &mut ExecutionParameters,
    ) -> Result<(), VerificationError> {
        if self.account_id() != executor_account_id {
            return Err(VerificationError::ExecutorAccountIdMismatch);
        }

        let p = self.parameters();
        if p.index != parameters.index {
            return Err(VerificationError::KeyIndexMismatch);
        }

        if p.nonce.0 != parameters.nonce.0 + 1 {
            return Err(VerificationError::NonceMismatch);
        }

        parameters.nonce.0 += 1;

        Ok(())
    }
}
