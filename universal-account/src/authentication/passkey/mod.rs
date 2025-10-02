use near_sdk::serde::de::DeserializeOwned;
use near_sdk::AccountId;
use near_sdk::{env, near};
use p256::ecdsa::signature::{SignerMut, Verifier};
use p256::ecdsa::{SigningKey, VerifyingKey};

use super::{ExecutionParameters, Key, VerifiablePayload};
use crate::Execute;

use data::{AuthenticatorData, ClientDataJson};
use signature::Signature;
use with_raw_string::WithRawString;

pub mod data;
pub mod signature;
pub mod with_raw_string;

fn sig_base(
    authenticator_data: &AuthenticatorData,
    client_data_json: &WithRawString<ClientDataJson>,
) -> Vec<u8> {
    [
        &**authenticator_data,
        &env::sha256(client_data_json.raw.as_bytes()),
    ]
    .concat()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct Passkey(pub crate::encoding::p256::PublicKey);

impl<T: Execute> Key<Message<T>> for Passkey {
    type Error = Error;

    fn verify_and_execute(&self, message: &Message<T>) -> Result<T::Output, Error> {
        // Check signature
        VerifyingKey::from(*self.0)
            .verify(
                &sig_base(&message.authenticator_data, &message.client_data_json),
                &*message.signature,
            )
            .map_err(|_| Error::InvalidSignature)?;

        // Check that the payload actually hashes to the signed challenge
        if message.message.hash() != message.client_data_json.parsed.challenge.as_slice() {
            return Err(Error::PayloadHashMismatch);
        }

        Ok(message.payload().execute())
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct Payload<T> {
    pub parameters: ExecutionParameters,
    pub account_id: AccountId,
    pub payload: T,
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(bound = "T: DeserializeOwned")]
pub struct Message<T> {
    pub authenticator_data: AuthenticatorData,
    pub message: WithRawString<Payload<T>>,
    pub client_data_json: WithRawString<ClientDataJson>,
    pub signature: Signature,
}

impl<T> Message<T> {
    pub fn new_and_sign(
        key: &p256::SecretKey,
        message: WithRawString<Payload<T>>,
        authenticator_data: AuthenticatorData,
        client_data_json: WithRawString<ClientDataJson>,
    ) -> Self {
        let signature = Signature(
            SigningKey::from(key).sign(&sig_base(&authenticator_data, &client_data_json)),
        );

        Self {
            authenticator_data,
            message,
            client_data_json,
            signature,
        }
    }
}

impl<T: Execute> VerifiablePayload for Message<T> {
    type Payload = T;

    fn account_id(&self) -> &near_sdk::AccountIdRef {
        &self.message.parsed.account_id
    }

    fn parameters(&self) -> &ExecutionParameters {
        &self.message.parsed.parameters
    }

    fn payload(&self) -> &Self::Payload {
        &self.message.parsed.payload
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Payload hash mismatch")]
    PayloadHashMismatch,
    #[error("Invalid signature")]
    InvalidSignature,
}
