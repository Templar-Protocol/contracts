use near_sdk::env;

use crate::{
    authentication::{verify_key, HashForSigning, Key},
    encoding,
};

pub type Message<T> = super::Message<VerifyKey, T>;

verify_key!(encoding::ed25519::PublicKey);

impl super::Ed25519Variant for VerifyKey {
    const PREFIX: &'static [u8] = b"\x19UAccount Signed Message:\n";
}

impl<T> Key<Message<T>> for VerifyKey {
    fn check_signature(
        &self,
        mws: &super::MessageWithSignature<Message<T>>,
    ) -> Result<(), super::CheckSignatureError> {
        let preimage = mws.message.preimage_for_signing();
        env::ed25519_verify(&mws.signature, &preimage, &self.0)
            .then_some(())
            .ok_or(super::CheckSignatureError::InvalidSignature)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::{
        authentication::{MessageWithSignature, Payload},
        KeyParameters, PayloadExecutionParameters, NEAR_TESTNET_CHAIN_ID,
    };

    use super::*;

    use near_sdk::{env, json_types::U64, AccountId};
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
        let message = Message::from_parsed(Payload::new(
            PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
                .with_key_parameters(KeyParameters {
                    block_height: U64(12345),
                    index: U64(0),
                    nonce: U64(0),
                })
                .verifying_contract(AccountId::from_str("account.near").unwrap())
                .build_salt(),
            "Hello, world!",
        ));

        let sol_sig = *keypair
            .sign_message(&message.preimage_for_signing())
            .as_array();

        let message = MessageWithSignature {
            message,
            signature: sol_sig.into(),
            auxiliary: (),
        };

        let key = VerifyKey((*keypair.pubkey().as_array()).into());

        key.verify_signature(message).unwrap();
    }

    #[rstest]
    #[test]
    #[should_panic = "InvalidSignature"]
    fn invalid_signature(keypair: Keypair) {
        let message = Message::from_parsed(Payload::new(
            PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
                .with_key_parameters(KeyParameters {
                    block_height: U64(12345),
                    index: U64(0),
                    nonce: U64(0),
                })
                .verifying_contract(AccountId::from_str("account.near").unwrap())
                .build_salt(),
            "Hello, world!",
        ));

        let sol_sig = *keypair
            .sign_message(&message.preimage_for_signing())
            .as_array();

        let key = VerifyKey((*keypair.pubkey().as_array()).into());

        let mws = MessageWithSignature {
            message: Message::from_parsed(Payload::new(
                PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
                    .with_key_parameters(KeyParameters {
                        block_height: U64(12345),
                        index: U64(0),
                        nonce: U64(1),
                    })
                    .verifying_contract(AccountId::from_str("account.near").unwrap())
                    .build_salt(),
                "Hello, world!",
            )),
            signature: sol_sig.into(),
            auxiliary: (),
        };

        key.verify_signature(mws).unwrap();
    }
}
