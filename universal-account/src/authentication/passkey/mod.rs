use near_sdk::{env, near, Promise};
use p256::ecdsa::signature::SignerMut;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{SigningKey, VerifyingKey};

use super::ExecutionParameters;
use super::Key;
use super::SignedMessage;
use crate::transaction::Transaction;

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
pub struct Passkey(pub crate::key::p256::PublicKey);

impl Key for Passkey {
    type Message = Message;
    type Error = Error;

    fn verify_and_execute(&self, message: &Message) -> Result<Promise, Error> {
        // Check signature
        VerifyingKey::from(*self.0)
            .verify(
                &sig_base(&message.authenticator_data, &message.client_data_json),
                &*message.signature,
            )
            .map_err(|_| Error::InvalidSignature)?;

        // Check that the payload actually hashes to the signed challenge
        if message.payload.hash() != message.client_data_json.parsed.challenge.as_slice() {
            return Err(Error::PayloadHashMismatch);
        }

        Ok(message.execute())
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct Message {
    pub authenticator_data: AuthenticatorData,
    pub payload: WithRawString<Transaction>,
    pub client_data_json: WithRawString<ClientDataJson>,
    pub signature: Signature,
}

impl Message {
    pub fn new_and_sign(
        key: &p256::SecretKey,
        payload: WithRawString<Transaction>,
        authenticator_data: AuthenticatorData,
        client_data_json: WithRawString<ClientDataJson>,
    ) -> Self {
        let signature = Signature(
            SigningKey::from(key).sign(&sig_base(&authenticator_data, &client_data_json)),
        );

        Self {
            authenticator_data,
            payload,
            client_data_json,
            signature,
        }
    }
}

impl SignedMessage for Message {
    type Output = Promise;

    fn account_id(&self) -> &near_sdk::AccountIdRef {
        &self.payload.parsed.account_id
    }

    fn parameters(&self) -> &ExecutionParameters {
        &self.payload.parsed.parameters
    }

    fn execute(&self) -> Promise {
        self.payload.parsed.construct_promise()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Payload hash mismatch")]
    PayloadHashMismatch,
    #[error("Invalid signature")]
    InvalidSignature,
}
