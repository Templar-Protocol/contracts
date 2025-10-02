use near_sdk::AccountIdRef;

use crate::{Execute, ExecutionParameters};

pub mod passkey;

pub trait Key<S: VerifiablePayload> {
    type Error: ToString;

    /// # Errors
    ///
    /// - If checking the signature fails
    fn verify_and_execute(
        &self,
        message: &S,
    ) -> Result<<S::Payload as Execute>::Output, Self::Error>;
}

pub trait VerifiablePayload {
    type Payload: Execute;

    fn account_id(&self) -> &AccountIdRef;
    fn parameters(&self) -> &ExecutionParameters;
    fn payload(&self) -> &Self::Payload;
}
