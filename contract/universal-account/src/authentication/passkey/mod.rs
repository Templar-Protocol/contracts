use data::{AuthenticatorData, ClientDataJson};
use near_sdk::{base64::prelude::*, env, near, Promise};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::VerifyingKey;
use signature::Signature;
use with_raw_string::WithRawString;

use crate::transaction::Transaction;

use super::{Executor, Nonce};

mod data;
mod signature;
mod with_raw_string;

#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct Input {
    authenticator_data: AuthenticatorData,
    payload: WithRawString<Transaction>,
    client_data_json: WithRawString<ClientDataJson>,
    signature: Signature,
}

impl Nonce for Input {
    fn nonce(&self) -> u64 {
        self.payload.parsed.nonce.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct Passkey {
    public_key: crate::key::p256::PublicKey,
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

impl Executor for Passkey {
    type Input = Input;
    type Error = Error;

    fn execute(&self, input: &Input) -> Result<Promise, Self::Error> {
        // Check signature
        let sig_base = [
            &*input.authenticator_data,
            &env::sha256(input.client_data_json.raw.as_bytes()),
        ]
        .concat();

        VerifyingKey::from(self.public_key.0)
            .verify(&sig_base, &*input.signature)
            .map_err(|_| Error::InvalidSignature)?;

        // Check that the un-hashed payload we received hashes to the value that was signed.
        let payload_hash = BASE64_STANDARD_NO_PAD
            .decode(&input.client_data_json.parsed.challenge)
            .map_err(|_| Error::InvalidChallenge)?;

        if env::sha256(input.payload.raw.as_bytes()) != payload_hash {
            return Err(Error::PayloadHashMismatch);
        }

        Ok(input.payload.parsed.construct_promise())
    }
}
