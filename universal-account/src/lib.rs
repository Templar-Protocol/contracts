use std::fmt::Display;

use alloy::sol;
use authentication::{
    ed25519_raw, eip712,
    passkey::{self, Passkey},
    CheckSignatureError, ExecutionContextProvider, ExecutionError, Key, MessageWithSignature,
};
use near_sdk::{json_types::U64, near, serde::de::DeserializeOwned, AccountIdRef};

pub mod authentication;
pub mod encoding;
mod event;
pub use event::Event;
pub mod transaction;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct InitArgs {
    pub key: KeyId,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum KeyId {
    Passkey(Passkey),
    Ed25519RawKey(ed25519_raw::VerifyKey),
    Eip712(eip712::VerifyKey),
}

impl Display for KeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passkey(passkey) => write!(f, "{}", passkey.0),
            Self::Ed25519RawKey(ed25519_raw_key) => write!(f, "{}", ed25519_raw_key.0),
            Self::Eip712(key) => write!(f, "{}", key.0),
        }
    }
}

impl From<Passkey> for KeyId {
    fn from(value: Passkey) -> Self {
        Self::Passkey(value)
    }
}

impl From<ed25519_raw::VerifyKey> for KeyId {
    fn from(value: ed25519_raw::VerifyKey) -> Self {
        Self::Ed25519RawKey(value)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
#[serde(deny_unknown_fields)]
pub struct ExecutionParameters {
    /// Static. If a universal account is deleted and recreated with the same
    /// keys, this ensures that old signatures are not replayable.
    pub block_height: U64,
    /// Static. If a key is deleted and re-added to the same account, this
    /// ensures that that old signatures are not replayable.
    pub index: U64,
    /// Increments for each message executed by this key.
    pub nonce: U64,
}

sol! {
    #[derive(Debug, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct SolExecutionParameters {
        uint64 block_height;
        uint64 index;
        uint64 nonce;
    }
}

impl From<SolExecutionParameters> for ExecutionParameters {
    fn from(value: SolExecutionParameters) -> Self {
        Self {
            block_height: U64(value.block_height),
            index: U64(value.index),
            nonce: U64(value.nonce),
        }
    }
}

impl From<ExecutionParameters> for SolExecutionParameters {
    fn from(value: ExecutionParameters) -> Self {
        Self {
            block_height: value.block_height.0,
            index: value.index.0,
            nonce: value.nonce.0,
        }
    }
}

impl ExecutionParameters {
    #[must_use]
    pub fn next(&self) -> Self {
        Self {
            nonce: U64(self.nonce.0 + 1),
            ..*self
        }
    }
}

#[derive(Debug, Clone)]
#[near(serializers = [json])]
#[serde(bound = "T: DeserializeOwned")]
pub enum ExecuteArgs<T> {
    Passkey {
        key: Passkey,
        message: Box<MessageWithSignature<passkey::Message<T>>>,
    },
    Ed25519Raw {
        key: ed25519_raw::VerifyKey,
        message: Box<MessageWithSignature<ed25519_raw::Message<T>>>,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq, PartialOrd, Ord)]
pub enum VerificationError {
    #[error(transparent)]
    Signature(#[from] CheckSignatureError),
    #[error(transparent)]
    Execution(#[from] ExecutionError),
}

impl<T> ExecuteArgs<T> {
    pub fn key_id(&self) -> KeyId {
        match self {
            Self::Passkey { key, .. } => KeyId::Passkey(key.clone()),
            Self::Ed25519Raw { key, .. } => KeyId::Ed25519RawKey(key.clone()),
        }
    }

    pub fn message_unchecked(&self) -> &T {
        match self {
            Self::Passkey { message, .. } => &message.message.0.parsed.payload,
            Self::Ed25519Raw { message, .. } => &message.message.0.parsed.payload,
        }
    }

    /// # Errors
    ///
    /// - If signature verification fails
    /// - If execution parameters do not match
    pub fn verify(
        self,
        executor_account_id: &AccountIdRef,
        parameters: &ExecutionParameters,
        allowed_origin: impl FnOnce(Option<&str>) -> bool,
    ) -> Result<T, VerificationError> {
        Ok(match self {
            ExecuteArgs::Passkey { key, message } => key
                .verify_signature(*message)?
                .verify_execution(executor_account_id, parameters, allowed_origin)?,
            ExecuteArgs::Ed25519Raw { key, message } => key
                .verify_signature(*message)?
                .verify_execution(executor_account_id, parameters, allowed_origin)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use authentication::{
        ed25519_raw,
        passkey::{
            self,
            data::{AuthenticatorData, ClientDataJson},
        },
        HashForSigning, Payload,
    };
    use near_sdk::{bs58, AccountId, NearToken};
    use p256::elliptic_curve::rand_core::OsRng;
    use rstest::rstest;
    use solana_sdk::{signature::Keypair, signer::Signer};
    use transaction::Transaction;

    use super::*;

    #[test]
    fn keyid_serialization() {
        let sk_p256 = p256::SecretKey::random(&mut OsRng);
        let passkey = Passkey(sk_p256.public_key().into());
        let passkey_id: KeyId = passkey.into();
        let passkey_id_str = passkey_id.to_string();

        let Some(b) = passkey_id_str.strip_prefix("p256:") else {
            panic!("invalid prefix");
        };

        let b = bs58::decode(b).into_vec().unwrap();
        assert_eq!(b.len(), 65);

        let sk_ed25519 = Keypair::new();
        let ed25519_raw = ed25519_raw::VerifyKey(sk_ed25519.pubkey().to_bytes().into());
        let ed25519_raw_id: KeyId = ed25519_raw.into();
        let ed25519_raw_id_str = ed25519_raw_id.to_string();

        let Some(b) = ed25519_raw_id_str.strip_prefix("ed25519:") else {
            panic!("invalid prefix");
        };

        let b = bs58::decode(b).into_vec().unwrap();
        assert_eq!(b.len(), 32);
    }

    fn payload() -> Payload<Box<[Transaction]>> {
        let payload = vec![Transaction {
            receiver_id: "token.near".parse().unwrap(),
            actions: vec![transaction::FunctionCallAction::new(
                "ft_transfer",
                br#"{"receiver_id":"receiver.near","amount":"100"}"#,
                NearToken::from_yoctonear(1),
                near_sdk::Gas::from_tgas(30),
            )
            .into()]
            .into_boxed_slice(),
        }]
        .into_boxed_slice();

        Payload {
            parameters: ExecutionParameters {
                block_height: U64(12345),
                index: U64(1),
                nonce: U64(44),
            },
            account_id: "my-universal-account.near".parse().unwrap(),
            payload,
        }
    }

    fn ed25519_raw_execute_args() -> ExecuteArgs<Box<[Transaction]>> {
        let sk = Keypair::new();

        let message = ed25519_raw::Message::from_parsed(payload());
        let preimage = message.preimage_for_signing();
        let signed_message = message.with_signature(sk.sign_message(&preimage).into());

        ExecuteArgs::Ed25519Raw {
            key: ed25519_raw::VerifyKey(sk.pubkey().to_bytes().into()),
            message: Box::new(signed_message),
        }
    }

    fn passkey_execute_args() -> ExecuteArgs<Box<[Transaction]>> {
        let sk = p256::SecretKey::random(&mut OsRng);

        let message = passkey::Message::from_parsed(payload());
        let hash = message.hash_for_signing();
        let signed_message: MessageWithSignature<_> = message.sign(
            &sk,
            AuthenticatorData(vec![1u8; 32].into_boxed_slice()),
            ClientDataJson {
                r#type: "type".to_string(),
                challenge: hash.into(),
                origin: "origin".to_string(),
                cross_origin: None,
                top_origin: None,
            },
        );

        ExecuteArgs::Passkey {
            key: Passkey(sk.public_key().into()),
            message: Box::new(signed_message),
        }
    }

    #[rstest]
    #[case("my-universal-account.near", 12345, 1, 44)]
    #[should_panic = "Execution(ExecutorAccountIdMismatch)"]
    #[case("my-universal-account.nearx", 12345, 1, 44)]
    #[should_panic = "Execution(BlockHeightMismatch)"]
    #[case("my-universal-account.near", 12346, 1, 44)]
    #[should_panic = "Execution(KeyIndexMismatch)"]
    #[case("my-universal-account.near", 12345, 0, 44)]
    #[should_panic = "Execution(NonceMismatch)"]
    #[case("my-universal-account.near", 12345, 1, 45)]
    #[test]
    fn verify(
        #[values(passkey_execute_args(), ed25519_raw_execute_args())] exec_args: ExecuteArgs<
            Box<[Transaction]>,
        >,
        #[case] executor_account_id: AccountId,
        #[case] block_height: u64,
        #[case] index: u64,
        #[case] nonce: u64,
    ) {
        exec_args
            .verify(
                &executor_account_id,
                &ExecutionParameters {
                    block_height: U64(block_height),
                    index: U64(index),
                    nonce: U64(nonce),
                },
                |_| true,
            )
            .unwrap();
    }

    #[rstest]
    #[case("origin")]
    #[should_panic = "Execution(OriginUnknown)"]
    #[case("origin2")]
    #[test]
    fn verify_origin(#[case] allowed_origin: &str) {
        passkey_execute_args()
            .verify(
                &AccountId::from_str("my-universal-account.near").unwrap(),
                &ExecutionParameters {
                    block_height: U64(12345),
                    index: U64(1),
                    nonce: U64(44),
                },
                |o| o == Some(allowed_origin),
            )
            .unwrap();
    }
}
