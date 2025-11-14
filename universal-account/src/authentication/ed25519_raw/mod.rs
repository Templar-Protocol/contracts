use near_sdk::serde::de::DeserializeOwned;
use near_sdk::{near, AccountId};

use crate::{encoding, ExecutionParameters};

use super::with_raw_string::WithRawString;
use super::{ExecutionContextProvider, Key, MagicNumber};

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct Ed25519RawKey(pub encoding::ed25519::PublicKey);

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(deny_unknown_fields)]
pub struct Payload<T> {
    pub parameters: ExecutionParameters,
    pub account_id: AccountId,
    pub payload: T,
}

impl<T> MagicNumber for Payload<T> {
    const MAGIC_NUMBER: &'static [u8] = b"\x19UAccount Signed Message:\n";
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(bound = "T: DeserializeOwned")]
pub struct Message<T> {
    pub message: WithRawString<Payload<T>>,
    pub signature: encoding::ed25519::Signature,
}

#[derive(Debug)]
pub struct MessageWithValidSignature<T>(Message<T>);

impl<T> Key<Message<T>> for Ed25519RawKey {
    type Verified = MessageWithValidSignature<T>;

    fn verify_signature(
        &self,
        message: Message<T>,
    ) -> Result<Self::Verified, crate::authentication::InvalidSignatureError> {
        if self.0.verify(
            &message.message.bytes_with_magic_number(),
            &message.signature,
        ) {
            Ok(MessageWithValidSignature(message))
        } else {
            Err(super::InvalidSignatureError)
        }
    }
}

impl<P> ExecutionContextProvider for MessageWithValidSignature<P> {
    type Payload = P;

    fn account_id(&self) -> &near_sdk::AccountIdRef {
        &self.0.message.parsed.account_id
    }

    fn parameters(&self) -> &ExecutionParameters {
        &self.0.message.parsed.parameters
    }

    fn payload_unchecked(self) -> Self::Payload {
        self.0.message.parsed.payload
    }

    fn origin(&self) -> Option<&str> {
        None
    }
}

#[cfg(test)]
mod tests {
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
        let message = WithRawString::from_parsed(Payload {
            parameters: ExecutionParameters {
                block_height: U64(12345),
                index: U64(0),
                nonce: U64(0),
            },
            account_id: "account.near".parse().unwrap(),
            payload: "Hello, world!",
        });

        let sol_sig = keypair.sign_message(&message.bytes_with_magic_number());

        let message = Message {
            message,
            signature: sol_sig.into(),
        };

        let key = Ed25519RawKey((*keypair.pubkey().as_array()).into());

        key.verify_signature(message).unwrap();
    }

    #[rstest]
    #[test]
    #[should_panic = "InvalidSignatureError"]
    fn invalid_signature(keypair: Keypair) {
        let message = WithRawString::from_parsed(Payload {
            parameters: ExecutionParameters {
                block_height: U64(12345),
                index: U64(0),
                nonce: U64(0),
            },
            account_id: "account.near".parse().unwrap(),
            payload: "Hello, world!",
        });

        let sol_sig = keypair.sign_message(&message.bytes_with_magic_number());

        let key = Ed25519RawKey((*keypair.pubkey().as_array()).into());

        key.verify_signature(Message {
            message: WithRawString::from_parsed(Payload {
                parameters: ExecutionParameters {
                    block_height: U64(12345),
                    index: U64(0),
                    nonce: U64(1),
                },
                account_id: "account.near".parse().unwrap(),
                payload: "Hello, world!",
            }),
            signature: sol_sig.into(),
        })
        .unwrap();
    }
}
