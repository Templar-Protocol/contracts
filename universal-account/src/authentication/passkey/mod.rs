use near_sdk::serde::de::DeserializeOwned;
use near_sdk::{env, near};
use p256::ecdsa::signature::{SignerMut, Verifier};
use p256::ecdsa::{SigningKey, VerifyingKey};

use super::with_raw_string::WithRawString;
use super::{ExecutionContextProvider, HashForSigning, InvalidSignatureError, Key, Payload};

use data::{AuthenticatorData, ClientDataJson};
use signature::Signature;

pub mod data;
pub mod signature;

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

impl std::fmt::Display for Passkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

pub struct MessageWithValidSignature<T>(MessageWithSignature<T>);

impl<T> Key<MessageWithSignature<T>> for Passkey {
    type Verified = MessageWithValidSignature<T>;

    fn verify_signature(
        &self,
        message: MessageWithSignature<T>,
    ) -> Result<Self::Verified, InvalidSignatureError> {
        let payload_prehash = sig_base(&message.0.authenticator_data, &message.0.client_data_json);
        if VerifyingKey::from(*self.0)
            .verify(&payload_prehash, &*message.0.signature)
            .is_ok()
        {
            Ok(MessageWithValidSignature(message))
        } else {
            Err(InvalidSignatureError)
        }
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(bound = "T: DeserializeOwned", deny_unknown_fields)]
pub struct Message<T>(pub WithRawString<Payload<T>>);

impl<T> Message<T> {
    pub fn from_parsed(payload: Payload<T>) -> Self
    where
        T: near_sdk::serde::Serialize,
    {
        Self(WithRawString::from_parsed(payload))
    }

    pub fn sign(
        self,
        key: &p256::SecretKey,
        authenticator_data: AuthenticatorData,
        client_data_json: ClientDataJson,
    ) -> MessageWithSignatureWithUncheckedHashes<T> {
        let client_data_json = WithRawString::from_parsed(client_data_json);
        let signature = Signature(
            SigningKey::from(key).sign(&sig_base(&authenticator_data, &client_data_json)),
        );

        MessageWithSignatureWithUncheckedHashes {
            authenticator_data,
            message: self,
            client_data_json,
            signature,
        }
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(bound = "T: DeserializeOwned", deny_unknown_fields)]
pub struct MessageWithSignatureWithUncheckedHashes<T> {
    pub authenticator_data: AuthenticatorData,
    pub message: Message<T>,
    pub client_data_json: WithRawString<ClientDataJson>,
    pub signature: Signature,
}

impl<T> HashForSigning for Message<T> {
    const MAGIC_NUMBER: &'static [u8] = b"\x19UAccount Signed Message:\n";

    fn content_bytes(&self) -> Vec<u8> {
        self.0.raw.as_bytes().to_vec()
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("Payload hash mismatch")]
pub struct PayloadHashMismatchError;

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(
    bound = "T: DeserializeOwned",
    try_from = "MessageWithSignatureWithUncheckedHashes<T>"
)]
pub struct MessageWithSignature<T>(MessageWithSignatureWithUncheckedHashes<T>);

impl<T> MessageWithSignature<T> {
    pub fn payload_unchecked(&self) -> &T {
        &self.0.message.0.parsed.payload
    }
}

impl<T> TryFrom<MessageWithSignatureWithUncheckedHashes<T>> for MessageWithSignature<T> {
    type Error = PayloadHashMismatchError;

    fn try_from(value: MessageWithSignatureWithUncheckedHashes<T>) -> Result<Self, Self::Error> {
        // Check that the payload actually hashes to the signed challenge
        if value.message.hash_for_signing() != value.client_data_json.parsed.challenge.as_slice() {
            return Err(PayloadHashMismatchError);
        }

        Ok(Self(value))
    }
}

impl<P> ExecutionContextProvider for MessageWithValidSignature<P> {
    type Payload = P;

    fn payload(self) -> Payload<Self::Payload> {
        self.0 .0.message.0.parsed
    }

    fn origin(&self) -> Option<&str> {
        Some(self.0 .0.client_data_json.parsed.origin.as_str())
    }
}
