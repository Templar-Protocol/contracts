use data::{AuthenticatorData, ClientDataJson};
use near_sdk::{base64::prelude::*, env, near, Promise};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::VerifyingKey;
use signature::Signature;
use with_raw_string::WithRawString;

use crate::transaction::Transaction;

use super::SignedMessage;

mod data;
mod signature;
mod with_raw_string;

#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct Message {
    authenticator_data: AuthenticatorData,
    payload: WithRawString<Transaction>,
    client_data_json: WithRawString<ClientDataJson>,
    signature: Signature,
}

impl SignedMessage for Message {
    type Key = crate::key::p256::PublicKey;
    type Output = Promise;
    type Error = Error;

    fn nonce(&self) -> u64 {
        self.payload.parsed.nonce.0
    }

    fn execute(&self, key: &Self::Key) -> Result<Self::Output, Self::Error> {
        // Check signature
        let sig_base = [
            &*self.authenticator_data,
            &env::sha256(self.client_data_json.raw.as_bytes()),
        ]
        .concat();

        VerifyingKey::from(key.0)
            .verify(&sig_base, &*self.signature)
            .map_err(|_| Error::InvalidSignature)?;

        // Check that the un-hashed payload we received hashes to the value that was signed.
        let payload_hash = BASE64_STANDARD_NO_PAD
            .decode(&self.client_data_json.parsed.challenge)
            .map_err(|_| Error::InvalidChallenge)?;

        if env::sha256(self.payload.raw.as_bytes()) != payload_hash {
            return Err(Error::PayloadHashMismatch);
        }

        Ok(self.payload.parsed.construct_promise())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid challenge")]
    InvalidChallenge,
    #[error("Payload hash mismatch")]
    PayloadHashMismatch,
    #[error("Invalid signature")]
    InvalidSignature,
}
