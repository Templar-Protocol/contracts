use std::ops::Deref;

use near_sdk::{
    base64::prelude::*,
    env, near,
    serde::{self, de, Deserialize, Serialize},
    serde_json, Promise,
};
use p256::ecdsa;
use p256::ecdsa::signature::Verifier;

use crate::transaction::Transaction;

use super::{NonceExtractor, PayloadExecutor};

#[derive(Clone, Debug)]
#[near(serializers = [])]
pub struct AuthenticatorData(pub Vec<u8>);

impl Deref for AuthenticatorData {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AuthenticatorData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes_hex: String = Deserialize::deserialize(deserializer)?;
        let bytes = hex::decode(bytes_hex).map_err(de::Error::custom)?;
        Ok(Self(bytes))
    }
}

impl Serialize for AuthenticatorData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Serialize::serialize(&hex::encode(&self.0), serializer)
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [])]
pub struct WithRawString<T> {
    raw: String,
    parsed: T,
}

impl<'de, T: for<'a> Deserialize<'a>> Deserialize<'de> for WithRawString<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw: String = Deserialize::deserialize(deserializer)?;
        let mut d = serde_json::Deserializer::from_str(&r#raw);
        let parsed: T = Deserialize::deserialize(&mut d).map_err(de::Error::custom)?;

        Ok(Self { raw, parsed })
    }
}

impl<T> Serialize for WithRawString<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Serialize::serialize(&self.raw, serializer)
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
#[serde(rename_all = "camelCase")]
pub struct ClientDataJson {
    r#type: String,
    challenge: String,
    origin: String,
    cross_origin: Option<bool>,
    top_origin: Option<String>,
}

#[derive(Clone, Debug)]
#[near(serializers = [])]
pub struct Signature(pub ecdsa::DerSignature);

impl Deref for Signature {
    type Target = ecdsa::DerSignature;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let signature_b64url: String = serde::Deserialize::deserialize(deserializer)?;
        let signature_bytes = BASE64_URL_SAFE_NO_PAD
            .decode(signature_b64url)
            .map_err(de::Error::custom)?;
        let signature =
            ecdsa::DerSignature::try_from(signature_bytes.as_slice()).map_err(de::Error::custom)?;
        Ok(Self(signature))
    }
}

impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let signature_b64url = BASE64_URL_SAFE_NO_PAD.encode(self.0.as_bytes());
        Serialize::serialize(&signature_b64url, serializer)
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct Input {
    authenticator_data: AuthenticatorData,
    payload: WithRawString<Transaction>,
    client_data_json: WithRawString<ClientDataJson>,
    signature: Signature,
}

impl NonceExtractor for Input {
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

impl PayloadExecutor for Passkey {
    type Input = Input;
    type Error = Error;

    fn execute(&self, input: &Input) -> Result<Promise, Self::Error> {
        // Check signature
        let msg = [
            &*input.authenticator_data,
            &env::sha256(input.client_data_json.raw.as_bytes()),
        ]
        .concat();

        self.public_key
            .verify(&msg, &*input.signature)
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
