use near_sdk::AccountIdRef;

use crate::ExecutionParameters;

pub mod passkey;

pub trait Key {
    type Message: SignedMessage;
    type Error: ToString;

    /// # Errors
    ///
    /// - If checking the signature fails
    fn verify_and_execute(
        &self,
        message: &Self::Message,
    ) -> Result<<Self::Message as SignedMessage>::Output, Self::Error>;
}

pub trait SignedMessage {
    type Output;

    fn account_id(&self) -> &AccountIdRef;
    fn parameters(&self) -> &ExecutionParameters;
    fn execute(&self) -> Self::Output;
}
