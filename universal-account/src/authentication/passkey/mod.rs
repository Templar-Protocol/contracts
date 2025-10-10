use near_sdk::serde::de::DeserializeOwned;
use near_sdk::AccountId;
use near_sdk::{env, near};
use p256::ecdsa::signature::{SignerMut, Verifier};
use p256::ecdsa::{SigningKey, VerifyingKey};

use super::{ExecutionContextProvider, ExecutionParameters, InvalidSignatureError, Key};

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

pub struct MessageWithValidSignature<T>(Message<T>);

impl<T> Key<Message<T>> for Passkey {
    type Validated = MessageWithValidSignature<T>;

    fn verify(&self, message: Message<T>) -> Result<Self::Validated, InvalidSignatureError> {
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
pub struct Payload<T> {
    pub parameters: ExecutionParameters,
    pub account_id: AccountId,
    pub payload: T,
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(bound = "T: DeserializeOwned")]
pub struct UncheckedMessage<T> {
    pub authenticator_data: AuthenticatorData,
    pub message: WithRawString<Payload<T>>,
    pub client_data_json: WithRawString<ClientDataJson>,
    pub signature: Signature,
}

impl<T> UncheckedMessage<T> {
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

#[derive(Debug, Clone, thiserror::Error)]
#[error("Payload hash mismatch")]
pub struct PayloadHashMismatchError;

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(bound = "T: DeserializeOwned", try_from = "UncheckedMessage<T>")]
pub struct Message<T>(UncheckedMessage<T>);

impl<T> Message<T> {
    pub fn payload_unchecked(&self) -> &T {
        &self.0.message.parsed.payload
    }
}

impl<T> TryFrom<UncheckedMessage<T>> for Message<T> {
    type Error = PayloadHashMismatchError;

    fn try_from(value: UncheckedMessage<T>) -> Result<Self, Self::Error> {
        // Check that the payload actually hashes to the signed challenge
        if value.message.hash() != value.client_data_json.parsed.challenge.as_slice() {
            return Err(PayloadHashMismatchError);
        }

        Ok(Self(value))
    }
}

impl<P> ExecutionContextProvider for MessageWithValidSignature<P> {
    type Payload = P;

    fn account_id(&self) -> &near_sdk::AccountIdRef {
        &self.0 .0.message.parsed.account_id
    }

    fn parameters(&self) -> &ExecutionParameters {
        &self.0 .0.message.parsed.parameters
    }

    fn payload_unchecked(&self) -> &Self::Payload {
        &self.0 .0.message.parsed.payload
    }
}
