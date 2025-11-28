use near_sdk::near;
use near_sdk::serde::de::DeserializeOwned;

use crate::encoding;

use super::with_raw_string::WithRawString;
use super::{
    CheckSignatureError, ExecutionContextProvider, HashForSigning, Key, MessageWithSignature,
    MessageWithValidSignature, Payload,
};

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct VerifyKey(pub encoding::ed25519::PublicKey);

impl std::fmt::Display for VerifyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json])]
#[serde(bound = "T: DeserializeOwned")]
pub struct Message<T>(pub WithRawString<Payload<T>>);

impl<T> super::SignableMessage for Message<T> {
    type Key = VerifyKey;
    type Signature = encoding::ed25519::Signature;
}

impl<T: near_sdk::serde::Serialize> Message<T> {
    pub fn from_parsed(payload: Payload<T>) -> Self {
        Self(WithRawString::from_parsed(payload))
    }

    pub fn with_signature(
        self,
        signature: encoding::ed25519::Signature,
    ) -> MessageWithSignature<Self> {
        MessageWithSignature {
            message: self,
            signature,
        }
    }
}

impl<T> From<WithRawString<Payload<T>>> for Message<T> {
    fn from(value: WithRawString<Payload<T>>) -> Self {
        Self(value)
    }
}

impl<T> HashForSigning for Message<T> {
    const MAGIC_NUMBER: &'static [u8] = b"\x19UAccount Signed Message:\n";

    fn content_bytes(&self) -> Vec<u8> {
        self.0.raw.as_bytes().to_vec()
    }
}

impl<T> Key<Message<T>> for VerifyKey {
    fn check_signature(
        &self,
        message: &super::MessageWithSignature<Message<T>>,
    ) -> Result<(), CheckSignatureError> {
        (self.0)
            .verify(&message.message.preimage_for_signing(), &message.signature)
            .then_some(())
            .ok_or(CheckSignatureError::InvalidSignature)
    }
}

impl<P> ExecutionContextProvider for MessageWithValidSignature<Message<P>> {
    type Payload = P;

    fn payload(self) -> Payload<Self::Payload> {
        self.0.message.0.parsed
    }

    fn origin(&self) -> Option<&str> {
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::{KeyParameters, PayloadExecutionParameters, NEAR_TESTNET_CHAIN_ID};

    use super::*;

    use near_sdk::{env, json_types::U64};
    use rstest::{fixture, rstest};
    use solana_sdk::{signature::Keypair, signer::Signer};

    #[fixture]
    fn keypair() -> Keypair {
        let keypair_bytes = [
            174, 47, 154, 16, 202, 193, 206, 113, 199, 190, 53, 133, 169, 175, 31, 56, 222, 53,
            138, 189, 224, 216, 117, 173, 10, 149, 53, 45, 73, 251, 237, 246, 15, 185, 186, 82,
            177, 240, 148, 69, 241, 227, 167, 80, 141, 89, 240, 121, 121, 35, 172, 247, 68, 251,
            226, 218, 48, 63, 176, 109, 168, 89, 238, 135,
        ];
        Keypair::try_from(&keypair_bytes[..]).unwrap()
    }

    #[rstest]
    #[test]
    fn solana_and_near_agree(keypair: Keypair) {
        let message = "The quick brown fox jumps over the lazy dog";

        let signature = keypair.sign_message(message.as_bytes());
        let is_valid_signature = signature.verify(&keypair.pubkey().to_bytes(), message.as_bytes());
        assert!(is_valid_signature);

        let near_verify = env::ed25519_verify(
            signature.as_array(),
            message.as_bytes(),
            keypair.pubkey().as_array(),
        );
        assert!(near_verify);
    }

    #[rstest]
    #[test]
    fn valid_signature(keypair: Keypair) {
        let message: Message<_> = WithRawString::from_parsed(Payload::new(
            PayloadExecutionParameters::new_auto(
                "account.near".parse().unwrap(),
                KeyParameters {
                    block_height: U64(12345),
                    index: U64(0),
                    nonce: U64(0),
                },
                NEAR_TESTNET_CHAIN_ID,
            ),
            "Hello, world!",
        ))
        .into();

        let sol_sig = keypair.sign_message(&message.preimage_for_signing());

        let message = MessageWithSignature {
            message,
            signature: sol_sig.into(),
        };

        let key = VerifyKey((*keypair.pubkey().as_array()).into());

        key.verify_signature(message).unwrap();
    }

    #[rstest]
    #[test]
    #[should_panic = "InvalidSignature"]
    fn invalid_signature(keypair: Keypair) {
        let message: Message<_> = WithRawString::from_parsed(Payload::new(
            PayloadExecutionParameters::new_auto(
                "account.near".parse().unwrap(),
                KeyParameters {
                    block_height: U64(12345),
                    index: U64(0),
                    nonce: U64(0),
                },
                NEAR_TESTNET_CHAIN_ID,
            ),
            "Hello, world!",
        ))
        .into();

        let sol_sig = keypair.sign_message(&message.preimage_for_signing());

        let key = VerifyKey((*keypair.pubkey().as_array()).into());

        let mws = MessageWithSignature {
            message: Message(WithRawString::from_parsed(Payload::new(
                PayloadExecutionParameters::new_auto(
                    "account.near".parse().unwrap(),
                    KeyParameters {
                        block_height: U64(12345),
                        index: U64(0),
                        nonce: U64(1),
                    },
                    NEAR_TESTNET_CHAIN_ID,
                ),
                "Hello, world!",
            ))),
            signature: sol_sig.into(),
        };

        key.verify_signature(mws).unwrap();
    }
}
