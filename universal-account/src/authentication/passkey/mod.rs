use near_sdk::serde::de::DeserializeOwned;
use near_sdk::{env, near};
use p256::ecdsa::signature::{SignerMut, Verifier};
use p256::ecdsa::{SigningKey, VerifyingKey};

use super::with_raw_string::WithRawString;
use super::{
    CheckSignatureError, ExecutionContextProvider, HashForSigning, Key, MessageWithSignature,
    MessageWithValidSignature, Payload,
};

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

impl<T> Key<Message<T>> for Passkey {
    fn check_signature(
        &self,
        mws: &super::MessageWithSignature<Message<T>>,
    ) -> Result<(), CheckSignatureError> {
        if mws.message.hash_for_signing()
            != mws.signature.client_data_json.parsed.challenge.as_slice()
        {
            return Err(CheckSignatureError::Other(
                "Computed hash does not match clientDataJson.challenge".into(),
            ));
        }

        let payload_prehash = sig_base(
            &mws.signature.authenticator_data,
            &mws.signature.client_data_json,
        );

        if VerifyingKey::from(*self.0)
            .verify(&payload_prehash, &*mws.signature.signature)
            .is_ok()
        {
            Ok(())
        } else {
            Err(CheckSignatureError::InvalidSignature)
        }
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(bound = "T: DeserializeOwned", deny_unknown_fields)]
pub struct Message<T>(pub WithRawString<Payload<T>>);

impl<T> super::SignableMessage for Message<T> {
    type Key = Passkey;
    type Signature = PasskeySignatureData;
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct PasskeySignatureData {
    pub authenticator_data: AuthenticatorData,
    pub client_data_json: WithRawString<ClientDataJson>,
    pub signature: Signature,
}

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
    ) -> MessageWithSignature<Self> {
        let client_data_json = WithRawString::from_parsed(client_data_json);
        let signature = Signature(
            SigningKey::from(key).sign(&sig_base(&authenticator_data, &client_data_json)),
        );

        MessageWithSignature {
            message: self,
            signature: PasskeySignatureData {
                authenticator_data,
                client_data_json,
                signature,
            },
        }
    }
}

impl<T> HashForSigning for Message<T> {
    const MAGIC_NUMBER: &'static [u8] = b"\x19UAccount Signed Message:\n";

    fn content_bytes(&self) -> Vec<u8> {
        self.0.raw.as_bytes().to_vec()
    }
}

impl<P> ExecutionContextProvider for MessageWithValidSignature<Message<P>> {
    type Payload = P;

    fn payload(self) -> Payload<Self::Payload> {
        self.0.message.0.parsed
    }

    fn origin(&self) -> Option<&str> {
        Some(self.0.signature.client_data_json.parsed.origin.as_str())
    }
}

#[cfg(test)]
mod tests {
    use crate::ExecutionParameters;

    use super::*;

    fn signer() -> p256::SecretKey {
        p256::SecretKey::from_bytes(
            &[
                70, 48, 167, 111, 21, 39, 70, 169, 116, 174, 182, 153, 87, 171, 62, 105, 44, 200,
                187, 37, 7, 63, 69, 153, 92, 134, 167, 206, 188, 7, 118, 35,
            ]
            .into(),
        )
        .unwrap()
    }

    fn message() -> Message<String> {
        Message::from_parsed(Payload {
            parameters: ExecutionParameters::default(),
            account_id: "account_id".parse().unwrap(),
            payload: "payload".to_string(),
        })
    }

    fn authenticator_data() -> AuthenticatorData {
        AuthenticatorData([0u8; 32].into())
    }

    fn client_data_json(challenge: data::Challenge) -> ClientDataJson {
        ClientDataJson {
            r#type: "type".to_string(),
            challenge,
            origin: "origin".to_string(),
            cross_origin: None,
            top_origin: None,
        }
    }

    #[test]
    fn check_signature() {
        let signer = signer();
        let message = message();

        let challenge = data::Challenge(message.hash_for_signing());

        let mws = message.sign(&signer, authenticator_data(), client_data_json(challenge));

        let verify_key = Passkey(signer.public_key().into());
        verify_key.verify_signature(mws).unwrap();
    }

    #[test]
    #[should_panic = "InvalidSignatureError"]
    fn check_signature_fail_hash_mismatch() {
        let signer = signer();
        let message = message();

        let challenge = data::Challenge([0x99_u8; 32]);

        let mws = message.sign(&signer, authenticator_data(), client_data_json(challenge));

        let verify_key = Passkey(signer.public_key().into());
        verify_key.verify_signature(mws).unwrap();
    }
}
