use near_sdk::{
    near,
    serde::{self, de::DeserializeOwned},
};

use crate::{
    authentication::{
        ed25519::{eip191, raw, sep53},
        passkey, CheckSignatureError, ExecutionContextProvider, ExecutionError, Key,
        MessageWithSignature, MessageWithValidSignature, SignableMessage,
    },
    KeyId, PayloadExecutionParameters,
};

macro_rules! execute_args {
    ($( $n:ident ( $verify_key: ty, $message: ty ) ),*) => {
        #[derive(Debug, Clone)]
        #[near(serializers = [json])]
        #[serde(bound = "T: DeserializeOwned")]
        pub enum ExecuteArgs<T: serde::Serialize> {
            $(
                $n (ExecuteArgsMessage<$verify_key, $message>)
            ),*
        }

        $(
            impl<T: serde::Serialize> From<ExecuteArgsMessage<$verify_key, $message>>
                for ExecuteArgs<T>
            {
                fn from(value: ExecuteArgsMessage<$verify_key, $message>) -> Self {
                    Self::$n(value)
                }
            }
        )*

        impl<T: serde::Serialize> ExecuteArgs<T> {
            pub fn key_id(&self) -> KeyId {
                match self {
                    $( Self::$n(args) => KeyId::$n(args.key.clone()), )*
                }
            }

            pub fn message_unchecked(&self) -> &T {
                match self {
                    $( Self::$n(args) => args.mws.message.0.parsed.payload_ref(), )*
                }
            }

            /// # Errors
            ///
            /// - If signature verification fails
            /// - If execution parameters do not match
            pub fn verify(
                self,
                expected_parameters: &PayloadExecutionParameters,
                allowed_origin: impl FnOnce(Option<&str>) -> bool,
            ) -> Result<T, VerificationError> {
                match self {
                    $( Self::$n(args) => args.verify(expected_parameters, allowed_origin), )*
                }
            }
        }
    };
}

execute_args! {
    Passkey(passkey::VerifyKey, passkey::Message<T>),
    Ed25519Raw(raw::VerifyKey, raw::Message<T>),
    Sep53(sep53::VerifyKey, sep53::Message<T>),
    Eip191(eip191::VerifyKey, eip191::Message<T>)
}

#[derive(Debug, Clone)]
#[near(serializers = [json])]
enum ExecuteArgsMaybeFlattenedMessage<K, M: SignableMessage> {
    #[serde(untagged)]
    Flattened(ExecuteArgsFlattenedMessage<K, M>),
    #[serde(untagged)]
    Unflattened(ExecuteArgsUnflattenedMessage<K, M>),
}

#[derive(Debug, Clone)]
#[near(serializers = [json])]
struct ExecuteArgsUnflattenedMessage<K, M: SignableMessage> {
    pub key: K,
    pub message: Box<MessageWithSignature<M>>,
}

#[derive(Debug, Clone)]
#[near(serializers = [json])]
struct ExecuteArgsFlattenedMessage<K, M: SignableMessage> {
    pub key: K,
    #[serde(flatten)]
    pub mws: Box<MessageWithSignature<M>>,
}

#[derive(Debug, Clone)]
#[near(serializers = [json])]
#[serde(from = "ExecuteArgsMaybeFlattenedMessage<K, M>")]
pub struct ExecuteArgsMessage<K, M: SignableMessage> {
    pub key: K,
    #[serde(flatten)]
    pub mws: Box<MessageWithSignature<M>>,
}

impl<K: Key<M>, M: SignableMessage> ExecuteArgsMessage<K, M>
where
    MessageWithValidSignature<M>: ExecutionContextProvider,
{
    pub(crate) fn verify(
        self,
        expected_parameters: &PayloadExecutionParameters,
        allowed_origin: impl FnOnce(Option<&str>) -> bool,
    ) -> Result<
        <MessageWithValidSignature<M> as ExecutionContextProvider>::Payload,
        VerificationError,
    > {
        Ok(self
            .key
            .verify_signature(*self.mws)?
            .verify_execution(expected_parameters, allowed_origin)?)
    }
}

impl<K, M: SignableMessage> From<ExecuteArgsMaybeFlattenedMessage<K, M>>
    for ExecuteArgsMessage<K, M>
{
    fn from(value: ExecuteArgsMaybeFlattenedMessage<K, M>) -> Self {
        match value {
            ExecuteArgsMaybeFlattenedMessage::Flattened(flattened) => Self {
                key: flattened.key,
                mws: flattened.mws,
            },
            ExecuteArgsMaybeFlattenedMessage::Unflattened(unflattened) => Self {
                key: unflattened.key,
                mws: unflattened.message,
            },
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq, PartialOrd, Ord)]
pub enum VerificationError {
    #[error(transparent)]
    Signature(#[from] CheckSignatureError),
    #[error(transparent)]
    Execution(#[from] ExecutionError),
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use alloy::signers::local::PrivateKeySigner;
    use near_sdk::{json_types::U64, serde_json, AccountId, NearToken};
    use p256::elliptic_curve::rand_core::OsRng;
    use rstest::rstest;
    use solana_sdk::{signature::Keypair, signer::Signer};

    use crate::{
        authentication::{
            ed25519::raw,
            passkey::data::{AuthenticatorData, ClientDataJson},
            HashForSigning, Payload,
        },
        transaction::{self, Transaction},
        KeyParameters, NEAR_TESTNET_CHAIN_ID,
    };

    use super::*;

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

        Payload::new(
            PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
                .with_key_parameters(KeyParameters {
                    block_height: U64(12345),
                    index: U64(1),
                    nonce: U64(44),
                })
                .verifying_contract(AccountId::from_str("my-universal-account.near").unwrap())
                .build_salt(),
            payload,
        )
    }

    fn eip191_execute_args() -> ExecuteArgs<Box<[Transaction]>> {
        let sk = PrivateKeySigner::random();

        let message = eip191::Message::from_parsed(payload());

        let signed_message = message.sign(&sk).unwrap();

        ExecuteArgsMessage {
            key: eip191::VerifyKey(sk.address().into()),
            mws: Box::new(signed_message),
        }
        .into()
    }

    fn ed25519_raw_execute_args() -> ExecuteArgs<Box<[Transaction]>> {
        let sk = Keypair::new();

        let message = raw::Message::from_parsed(payload());
        let preimage = message.preimage_for_signing();
        let signed_message =
            message.with_signature((*sk.sign_message(&preimage).as_array()).into());

        ExecuteArgsMessage {
            key: raw::VerifyKey(sk.pubkey().to_bytes().into()),
            mws: Box::new(signed_message),
        }
        .into()
    }

    fn sep53_execute_args() -> ExecuteArgs<Box<[Transaction]>> {
        let sk = Keypair::new();

        let message = sep53::Message::from_parsed(payload());
        let hash = message.hash_for_signing();
        let signed_message = message.with_signature((*sk.sign_message(&hash).as_array()).into());

        ExecuteArgsMessage {
            key: sep53::VerifyKey(sk.pubkey().to_bytes().into()),
            mws: Box::new(signed_message),
        }
        .into()
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

        ExecuteArgsMessage {
            key: passkey::VerifyKey(sk.public_key().into()),
            mws: Box::new(signed_message),
        }
        .into()
    }

    #[rstest]
    #[case("my-universal-account.near", 12345, 1, 44)]
    #[should_panic = r#"Execution(Mismatch { field: "verifying_contract", expected: "my-universal-account.nearx", actual: "my-universal-account.near" })"#]
    #[case("my-universal-account.nearx", 12345, 1, 44)]
    #[should_panic = r#"Execution(Mismatch { field: "block_height", expected: "12346", actual: "12345" })"#]
    #[case("my-universal-account.near", 12346, 1, 44)]
    #[should_panic = r#"Execution(Mismatch { field: "index", expected: "0", actual: "1" })"#]
    #[case("my-universal-account.near", 12345, 0, 44)]
    #[should_panic = r#"Execution(Mismatch { field: "nonce", expected: "45", actual: "44" })"#]
    #[case("my-universal-account.near", 12345, 1, 45)]
    #[test]
    fn verify(
        #[values(
            passkey_execute_args(),
            ed25519_raw_execute_args(),
            sep53_execute_args(),
            eip191_execute_args()
        )]
        exec_args: ExecuteArgs<Box<[Transaction]>>,
        #[case] executor_account_id: AccountId,
        #[case] block_height: u64,
        #[case] index: u64,
        #[case] nonce: u64,
    ) {
        exec_args
            .verify(
                &PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
                    .with_key_parameters(KeyParameters {
                        block_height: U64(block_height),
                        index: U64(index),
                        nonce: U64(nonce),
                    })
                    .verifying_contract(executor_account_id)
                    .build_salt(),
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
                &PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
                    .with_key_parameters(KeyParameters {
                        block_height: U64(12345),
                        index: U64(1),
                        nonce: U64(44),
                    })
                    .verifying_contract(AccountId::from_str("my-universal-account.near").unwrap())
                    .build_salt(),
                |o| o == Some(allowed_origin),
            )
            .unwrap();
    }

    #[rstest]
    #[case::ed25519raw_unflattened("e3ff9a0ab355.user0.tmplr.near", 173_342_352, 41, r#"{"Ed25519Raw":{"key":"ed25519:BTPUmzP1v4t7kNB69i4v8d1Ci5egN62Fs8QjePMSfJvo","message":{"message":"{\"parameters\":{\"block_height\":\"173342352\",\"index\":\"0\",\"nonce\":\"41\"},\"account_id\":\"e3ff9a0ab355.user0.tmplr.near\",\"payload\":[{\"receiver_id\":\"pyth-oracle.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"update_price_feeds\",\"arguments\":\"eyJkYXRhIjoiNTA0ZTQxNTUwMTAwMDAwMDAzYjgwMTAwMDAwMDA0MGQwMDVkY2EyNzdhYjk1NjViMjI0ZDZkY2NjZjBlNGVlMDdhZDM1NDhhMDViMTAwNGNjY2ZmMWI4YTlmMWFiNWZmOWQ0YzYxMzdhNWZlYTYxYThlNTFhOTZhZGM1Yjg4MjU0MzE3YWEwZjM1MzJkN2ZmY2VkZGMwNWE1ZGUwZDlhNjRjMDAwMzM5MmU5MTk1NDVjM2Q1Y2JmYWVmOGUxNDkxMzlmY2JiMzQ3ZjI4YTU5ZDVhNWRhZTE3Y2VhNjFjMjFjNDU0N2I1NmY3MzQyNzVkMzAwNWQ1YTYyN2I3YTMwZmNlMjBmOTA5MjNjMjU3MWY4Yjk4MGUxYjVjNjQ4YjQ4Mjg0M2U0MDEwNGJhOTRmOWFiMzcxMjY3ODdlOTdlZmNkODlmMzU5OTdmYTc0NDI0N2FlNmQ5NTVlZGU3ZTVjNGFiNTE2ODA5N2YyZjNiMmM3ZTk1NmUzM2U3MzQzNDc3Mzk4YzhjNTZhN2U1YzM0MDFhZWFiNmY3ZDc4NmI1MWFmYzIzN2E5OGVhMDEwNjk4NTllMjMyNGFhNjM2YzI5NjU1YzJmOWQxMzgyZGExOTg4M2E2NWIyMzc1NWI4MzEzMmMzZDhhNDA4MjhiMDIxMTMzMTQxMmM0NzYxN2NlYWVkYTM3MTIxNjM2YmRkNjFjY2E2NzVkZWQwZTdmNjg1ZTc1OTJiYzY2OWIwNjI3MDAwODBmNzk1NTJhNjY4ZmM2YWEyMjVmNzkwODBjMmY1YzEzNmE2MDU1NjI3M2EwYzRjOWNlMTc2ODUzMDFiMGJlMTI1YWE2NTBjZDhlNzkxYTVhMGY0Yjg5ODdhZmZjNTFkMDdiMGYzNzcxMjAwMWUwNmI1MjRhYzkwMzI3Y2ZiMWFmMDAwYTczZTgxZjU1ODYwMGNiNGRkYTEwNTFjMWQ0MjBjNjQxNDgxNjVlZjM4MGVjNDU0ZDQwZjcwYTE4MTY5YWJmNzUyMWZkMzI4MWY5ZTM3YTRkMTM1YTU2Y2YwMDhjZDVkZGQ3ZmMzNTM2ZDU1M2QyODhlMmI0YjU5ZWRiMTA5NzVlMDAwYjEzYzI3YjA4Nzc5YTM4Nzk1ODdmMWVjMzNiNmU3YjVmNjgzY2I3NjRiOWM2YmVhNGEwMTExNmE4NjFjMjg1Yjc2MjhhMjUxMDk0NzI1YjFlOTY2Mjg3NThkYjg3YTg5MDY5OTQ5OTk4M2JiZTUxMmU2NGQxZjY2NTJkMDU1ZjQ0MDEwY2U3NWU3ZDFiNzRkZjFmMDRkYjZjZDhjNjU4Y2QwZmU5ODVjMjdkMjljYWM0OGU5MzI0ZDgxMGM5ZmZlYzJjMWEyYWQ2ZWYyYTRjNmRhZTMzMTIxYWY5NjUyZDJmYjk1YjZjN2FhZjA2MzQ3ZWE4OTU3ZDdhNzdhM2M5NGRjMTY3MDEwZDE3Yzk1NzBjZGZkOGI5Njc4ZjIyYTRlMGQzZDBmNWQ3ZTM1NzVmZjc1NzExODEyNTQzZDhlZWE3OWVjNzNkNWIzMmI0NTcwMDAwMDgyZjBiNTcwZmI4ZTRmMTFjOWE0ZTBiMmJkZGVlYmU0ZjQ4NDQxYzZjYWEyYzQ1ZDBmYTZmMDEwZTFkYjAyMWEzMzkyN2U1NmRlMGQ5OGJmMzZkMTUzNjEyMjM3N2NiNWU4NjQ3NzQ5NGJmMGYzZThhYWYyOWVhZTM3MGU2YmMzODc4ZWVmOTkxMjViMjk3NWU0OGY3MzhhYTg0MDNkOGEyZDhlMzFlNTMyMmY4NWNhZGFkZGMxMTEzMDAwZjFjZWIyYTBjZmU1ZTU0ZDRiYTIwMGRmY2Q2YzU3NmE3ZGViNzhkNGZiZTM5YTE4OWNlZDg1NDVjYWMzODYwMjg3MDE3NWQ1Y2M4YWZmNTRiMmNmNGRmZDUwYjc3YWFhMmRjZDk4YmVjZGEzODVlYWZhNGFhNjlkMzk1NDBiODJiMDAxMGVkNzBmNzY4OWQ1YTJkNzAxODJhZjVjMTAyYmZmZGI5ZGVmYzM2NDRiNzk0MGQ0ZmJmNDM4MzZiOWFkMzViZjM2ZGMwMTg0Y2Q4NTZjNDdmYWYzNWRlM2ZmYzg0YmExNzFkYWQxOWVmOTY2MTkxMDcyYWUxNmZlZmVhMzZiNmQ2MDAxMWJkY2RlOWExZWNmN2Q3ZjFlYjc3OTMzZDI5ZjA4MjFkNDE5NTFiZmFlYTA1YzEyZjc3ZmMyN2ZmNDA4YTA0NDc2ZGZiYmM0NWYwNWU3MmNiMGUwMzk4ZGIzOGMxZGM1OTZlY2IyMWJmNDhmN2I4NTU3MmE0N2Q5MjkyNzZlNGU1MDE2OTI5ODJkZjAwMDAwMDAwMDAxYWUxMDFmYWVkYWM1ODUxZTMyYjliMjNiNWY5NDExYThjMmJhYzRhYWUzZWQ0ZGQ3YjgxMWRkMWE3MmVhNGFhNzEwMDAwMDAwMDBhNGU0N2RlMDE0MTU1NTc1NjAwMDAwMDAwMDAwZjViZWViZjAwMDAyNzEwZTBiODlkNGFmNWYxMTA0MjJiZDUwMjM3ODdiYjFlMGE1MjMxODMxNDAyMDA1NTAwZWFhMDIwYzYxY2M0Nzk3MTI4MTM0NjFjZTE1Mzg5NGE5NmE2YzAwYjIxZWQwY2ZjMjc5OGQxZjlhOWU5Yzk0YTAwMDAwMDAwMDVmNTg5ZDYwMDAwMDAwMDAwMDEwNWQ1ZmZmZmZmZjgwMDAwMDAwMDY5Mjk4MmRmMDAwMDAwMDA2OTI5ODJkZjAwMDAwMDAwMDVmNThjOTIwMDAwMDAwMDAwMDExNjljMGRiOTVlZWM1NmYxMTkzNTZjY2YwZDdjNDg0ZmMyZTI0OTgzNTRjYjdmYzRjZTc2MTg5NzY0ZjJkMDNhODIwZTEwYmJhY2IwNzA0ZTJlNzAzNDYzNTE0NjVhZmY3Zjg2ZDQ1ZGUxY2FjMmQxYzhjMWEyMjk4NDljYTFkNjQ5ZmVjMGEwNWFiNDBmZTJjYzM0NWRkYjA2MGNmMjc0YjQ5ZmJlZmFjM2M1YjY5Njg2M2RlNzkwNDk2Nzc3NmRhMTRmYmFkMjJhODlhYjRmZGIyNzFmNzZjZWFjZjc2Y2M5MmQ0NDQ2NTgyZmE0NDU3NDlhZTliNTJhZWQxYTNiZWEzZjk4MWU4YjdkZmM0NjBjZDFlOWZlYmJjMWFjMDVjMjg0NzIzNGIwODZhY2U4MTRjZjQ0MTI0ZTM4OGExODllYTk0ODZjNTQ5N2ZkYzVjZTE4Y2Q0N2M4MDU5Njc4NTFiMTRhZDFjOGZkOWNmNTU0YjM5MjNmN2RhYjY5MmNjMjhkNzA4YTA2MDAwNzYzZmQyOWQxNDJjYWFjOGUxM2YwNTZmNWNmMDJhMDZjYmRjOTM1MzAzZjJmNGFiZmNhMzQ5NmY3OWNkY2MwYjZkZWY3Y2QxMjc0YWJmNjQ2NWMzZjM1N2UxNGNlNDc4MTlhNDVmOTcyZTk1ZTI0ZjYxYWY0ZjUxZjFmMjg0ZDQwMDA1NTAwYmU5YjU5ZDE3OGYwZDZhOTdhYjRjMzQzYmZmMmFhNjljYWExZWFhZTNlOTA0OGE2NTc4OGM1MjliMTI1YmIyNDAwMDAwMDBiMmRjNGQ3NTgwMDAwMDAwMDAzMjQ1ZDYxZmZmZmZmZjgwMDAwMDAwMDY5Mjk4MmRmMDAwMDAwMDA2OTI5ODJkZjAwMDAwMDBiMjZlZDM4YTgwMDAwMDAwMDAyYzE4Yzg0MGRhNDc0MTlkMTEyYWI4ZmI5Mjc1YjU2YmVhYzcxODkzMDlhMmY0ZjhmOGY2NjBmZmQ0YjMwMTIyZjRmN2ZkNTgyNjJjNjRkMTEyM2UwNjZmZDZkM2Q3NzAxYWQ1MjZmNDA5ZTMzNjE2NGI3Y2ZkYWM2NWQwZDZmYThkNDQ2M2I0MjFmYWYwMTQ1MjlkOWRmNWI1NDkxNWJlOTlkNDMzY2JiMDJjMGI2MGY3NGQzYzJhMTNiN2EyODQyOGZhYzE1N2ZkNTU0MTg2MzBiN2ZkNjk2ZWRiNjJmMTNhZGYzNjI4MDNjODUyMTYyZDg4NTQ1MmRhNzJkNzg0MGViMzQ1NjVhZjQ0MGM1OWUwOGJiYTAyYzk1ZThkZjdhYzQ2ZjY1MDg5ODg1OWU5NzI2MTAxNjIyNzA3ZjgxNTkyNzU0ZGVkMWQwYjc3OTBkOWFmODYyNjc2ODEyY2U1YTIwZjgxNGM0ZjBmM2M3ZGMyNDhlYmVjNTNlN2RiOWE3OTFlZjQ3YTAzYTY3NWQyOWE5MGNmYzYxNDJjYWFjOGUxM2YwNTZmNWNmMDJhMDZjYmRjOTM1MzAzZjJmNGFiZmNhMzQ5NmY3OWNkY2MwYjZkZWY3Y2QxMjc0YWJmNjQ2NWMzZjM1N2UxNGNlNDc4MTlhNDVmOTcyZTk1ZTI0ZjYxYWY0ZjUxZjFmMjg0ZDQwIn0=\",\"amount\":\"155000000000000\",\"gas\":\"155000000000000\"}}]},{\"receiver_id\":\"intents.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"mt_transfer_call\",\"arguments\":\"eyJyZWNlaXZlcl9pZCI6Iml6ZWMtaXNvbHVzZGMudGVtcGxhci1hbHBoYS5uZWFyIiwiYW1vdW50IjoiMTcyMTc2NyIsIm1zZyI6IlwiUmVwYXlcIiIsInRva2VuX2lkIjoibmVwMTQxOnNvbC01Y2UzYmYzYTMxYWYxOGJlNDBiYTMwZjcyMTEwMWI0MzQxNjkwMTg2Lm9tZnQubmVhciJ9\",\"amount\":\"1\",\"gas\":\"50000000000000\"}}]},{\"receiver_id\":\"izec-isolusdc.templar-alpha.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"withdraw_collateral\",\"arguments\":\"eyJhbW91bnQiOiI3MDUwMDAifQ==\",\"amount\":\"0\",\"gas\":\"50000000000000\"}}]}]}","signature":"ed25519:4CrFbqgiHo3BUv2QoHX7XDcRJqZeVvJJ8fB4SCgpzqSBPwsEyNCFT3uEKbwudDp2tzjQy9xPAeud6iq2qU1Crfet"}}}"#)]
    #[case::ed25519raw_flattened("e3ff9a0ab355.user0.tmplr.near", 173_342_352, 41, r#"{"Ed25519Raw":{"key":"ed25519:BTPUmzP1v4t7kNB69i4v8d1Ci5egN62Fs8QjePMSfJvo","message":"{\"parameters\":{\"block_height\":\"173342352\",\"index\":\"0\",\"nonce\":\"41\"},\"account_id\":\"e3ff9a0ab355.user0.tmplr.near\",\"payload\":[{\"receiver_id\":\"pyth-oracle.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"update_price_feeds\",\"arguments\":\"eyJkYXRhIjoiNTA0ZTQxNTUwMTAwMDAwMDAzYjgwMTAwMDAwMDA0MGQwMDVkY2EyNzdhYjk1NjViMjI0ZDZkY2NjZjBlNGVlMDdhZDM1NDhhMDViMTAwNGNjY2ZmMWI4YTlmMWFiNWZmOWQ0YzYxMzdhNWZlYTYxYThlNTFhOTZhZGM1Yjg4MjU0MzE3YWEwZjM1MzJkN2ZmY2VkZGMwNWE1ZGUwZDlhNjRjMDAwMzM5MmU5MTk1NDVjM2Q1Y2JmYWVmOGUxNDkxMzlmY2JiMzQ3ZjI4YTU5ZDVhNWRhZTE3Y2VhNjFjMjFjNDU0N2I1NmY3MzQyNzVkMzAwNWQ1YTYyN2I3YTMwZmNlMjBmOTA5MjNjMjU3MWY4Yjk4MGUxYjVjNjQ4YjQ4Mjg0M2U0MDEwNGJhOTRmOWFiMzcxMjY3ODdlOTdlZmNkODlmMzU5OTdmYTc0NDI0N2FlNmQ5NTVlZGU3ZTVjNGFiNTE2ODA5N2YyZjNiMmM3ZTk1NmUzM2U3MzQzNDc3Mzk4YzhjNTZhN2U1YzM0MDFhZWFiNmY3ZDc4NmI1MWFmYzIzN2E5OGVhMDEwNjk4NTllMjMyNGFhNjM2YzI5NjU1YzJmOWQxMzgyZGExOTg4M2E2NWIyMzc1NWI4MzEzMmMzZDhhNDA4MjhiMDIxMTMzMTQxMmM0NzYxN2NlYWVkYTM3MTIxNjM2YmRkNjFjY2E2NzVkZWQwZTdmNjg1ZTc1OTJiYzY2OWIwNjI3MDAwODBmNzk1NTJhNjY4ZmM2YWEyMjVmNzkwODBjMmY1YzEzNmE2MDU1NjI3M2EwYzRjOWNlMTc2ODUzMDFiMGJlMTI1YWE2NTBjZDhlNzkxYTVhMGY0Yjg5ODdhZmZjNTFkMDdiMGYzNzcxMjAwMWUwNmI1MjRhYzkwMzI3Y2ZiMWFmMDAwYTczZTgxZjU1ODYwMGNiNGRkYTEwNTFjMWQ0MjBjNjQxNDgxNjVlZjM4MGVjNDU0ZDQwZjcwYTE4MTY5YWJmNzUyMWZkMzI4MWY5ZTM3YTRkMTM1YTU2Y2YwMDhjZDVkZGQ3ZmMzNTM2ZDU1M2QyODhlMmI0YjU5ZWRiMTA5NzVlMDAwYjEzYzI3YjA4Nzc5YTM4Nzk1ODdmMWVjMzNiNmU3YjVmNjgzY2I3NjRiOWM2YmVhNGEwMTExNmE4NjFjMjg1Yjc2MjhhMjUxMDk0NzI1YjFlOTY2Mjg3NThkYjg3YTg5MDY5OTQ5OTk4M2JiZTUxMmU2NGQxZjY2NTJkMDU1ZjQ0MDEwY2U3NWU3ZDFiNzRkZjFmMDRkYjZjZDhjNjU4Y2QwZmU5ODVjMjdkMjljYWM0OGU5MzI0ZDgxMGM5ZmZlYzJjMWEyYWQ2ZWYyYTRjNmRhZTMzMTIxYWY5NjUyZDJmYjk1YjZjN2FhZjA2MzQ3ZWE4OTU3ZDdhNzdhM2M5NGRjMTY3MDEwZDE3Yzk1NzBjZGZkOGI5Njc4ZjIyYTRlMGQzZDBmNWQ3ZTM1NzVmZjc1NzExODEyNTQzZDhlZWE3OWVjNzNkNWIzMmI0NTcwMDAwMDgyZjBiNTcwZmI4ZTRmMTFjOWE0ZTBiMmJkZGVlYmU0ZjQ4NDQxYzZjYWEyYzQ1ZDBmYTZmMDEwZTFkYjAyMWEzMzkyN2U1NmRlMGQ5OGJmMzZkMTUzNjEyMjM3N2NiNWU4NjQ3NzQ5NGJmMGYzZThhYWYyOWVhZTM3MGU2YmMzODc4ZWVmOTkxMjViMjk3NWU0OGY3MzhhYTg0MDNkOGEyZDhlMzFlNTMyMmY4NWNhZGFkZGMxMTEzMDAwZjFjZWIyYTBjZmU1ZTU0ZDRiYTIwMGRmY2Q2YzU3NmE3ZGViNzhkNGZiZTM5YTE4OWNlZDg1NDVjYWMzODYwMjg3MDE3NWQ1Y2M4YWZmNTRiMmNmNGRmZDUwYjc3YWFhMmRjZDk4YmVjZGEzODVlYWZhNGFhNjlkMzk1NDBiODJiMDAxMGVkNzBmNzY4OWQ1YTJkNzAxODJhZjVjMTAyYmZmZGI5ZGVmYzM2NDRiNzk0MGQ0ZmJmNDM4MzZiOWFkMzViZjM2ZGMwMTg0Y2Q4NTZjNDdmYWYzNWRlM2ZmYzg0YmExNzFkYWQxOWVmOTY2MTkxMDcyYWUxNmZlZmVhMzZiNmQ2MDAxMWJkY2RlOWExZWNmN2Q3ZjFlYjc3OTMzZDI5ZjA4MjFkNDE5NTFiZmFlYTA1YzEyZjc3ZmMyN2ZmNDA4YTA0NDc2ZGZiYmM0NWYwNWU3MmNiMGUwMzk4ZGIzOGMxZGM1OTZlY2IyMWJmNDhmN2I4NTU3MmE0N2Q5MjkyNzZlNGU1MDE2OTI5ODJkZjAwMDAwMDAwMDAxYWUxMDFmYWVkYWM1ODUxZTMyYjliMjNiNWY5NDExYThjMmJhYzRhYWUzZWQ0ZGQ3YjgxMWRkMWE3MmVhNGFhNzEwMDAwMDAwMDBhNGU0N2RlMDE0MTU1NTc1NjAwMDAwMDAwMDAwZjViZWViZjAwMDAyNzEwZTBiODlkNGFmNWYxMTA0MjJiZDUwMjM3ODdiYjFlMGE1MjMxODMxNDAyMDA1NTAwZWFhMDIwYzYxY2M0Nzk3MTI4MTM0NjFjZTE1Mzg5NGE5NmE2YzAwYjIxZWQwY2ZjMjc5OGQxZjlhOWU5Yzk0YTAwMDAwMDAwMDVmNTg5ZDYwMDAwMDAwMDAwMDEwNWQ1ZmZmZmZmZjgwMDAwMDAwMDY5Mjk4MmRmMDAwMDAwMDA2OTI5ODJkZjAwMDAwMDAwMDVmNThjOTIwMDAwMDAwMDAwMDExNjljMGRiOTVlZWM1NmYxMTkzNTZjY2YwZDdjNDg0ZmMyZTI0OTgzNTRjYjdmYzRjZTc2MTg5NzY0ZjJkMDNhODIwZTEwYmJhY2IwNzA0ZTJlNzAzNDYzNTE0NjVhZmY3Zjg2ZDQ1ZGUxY2FjMmQxYzhjMWEyMjk4NDljYTFkNjQ5ZmVjMGEwNWFiNDBmZTJjYzM0NWRkYjA2MGNmMjc0YjQ5ZmJlZmFjM2M1YjY5Njg2M2RlNzkwNDk2Nzc3NmRhMTRmYmFkMjJhODlhYjRmZGIyNzFmNzZjZWFjZjc2Y2M5MmQ0NDQ2NTgyZmE0NDU3NDlhZTliNTJhZWQxYTNiZWEzZjk4MWU4YjdkZmM0NjBjZDFlOWZlYmJjMWFjMDVjMjg0NzIzNGIwODZhY2U4MTRjZjQ0MTI0ZTM4OGExODllYTk0ODZjNTQ5N2ZkYzVjZTE4Y2Q0N2M4MDU5Njc4NTFiMTRhZDFjOGZkOWNmNTU0YjM5MjNmN2RhYjY5MmNjMjhkNzA4YTA2MDAwNzYzZmQyOWQxNDJjYWFjOGUxM2YwNTZmNWNmMDJhMDZjYmRjOTM1MzAzZjJmNGFiZmNhMzQ5NmY3OWNkY2MwYjZkZWY3Y2QxMjc0YWJmNjQ2NWMzZjM1N2UxNGNlNDc4MTlhNDVmOTcyZTk1ZTI0ZjYxYWY0ZjUxZjFmMjg0ZDQwMDA1NTAwYmU5YjU5ZDE3OGYwZDZhOTdhYjRjMzQzYmZmMmFhNjljYWExZWFhZTNlOTA0OGE2NTc4OGM1MjliMTI1YmIyNDAwMDAwMDBiMmRjNGQ3NTgwMDAwMDAwMDAzMjQ1ZDYxZmZmZmZmZjgwMDAwMDAwMDY5Mjk4MmRmMDAwMDAwMDA2OTI5ODJkZjAwMDAwMDBiMjZlZDM4YTgwMDAwMDAwMDAyYzE4Yzg0MGRhNDc0MTlkMTEyYWI4ZmI5Mjc1YjU2YmVhYzcxODkzMDlhMmY0ZjhmOGY2NjBmZmQ0YjMwMTIyZjRmN2ZkNTgyNjJjNjRkMTEyM2UwNjZmZDZkM2Q3NzAxYWQ1MjZmNDA5ZTMzNjE2NGI3Y2ZkYWM2NWQwZDZmYThkNDQ2M2I0MjFmYWYwMTQ1MjlkOWRmNWI1NDkxNWJlOTlkNDMzY2JiMDJjMGI2MGY3NGQzYzJhMTNiN2EyODQyOGZhYzE1N2ZkNTU0MTg2MzBiN2ZkNjk2ZWRiNjJmMTNhZGYzNjI4MDNjODUyMTYyZDg4NTQ1MmRhNzJkNzg0MGViMzQ1NjVhZjQ0MGM1OWUwOGJiYTAyYzk1ZThkZjdhYzQ2ZjY1MDg5ODg1OWU5NzI2MTAxNjIyNzA3ZjgxNTkyNzU0ZGVkMWQwYjc3OTBkOWFmODYyNjc2ODEyY2U1YTIwZjgxNGM0ZjBmM2M3ZGMyNDhlYmVjNTNlN2RiOWE3OTFlZjQ3YTAzYTY3NWQyOWE5MGNmYzYxNDJjYWFjOGUxM2YwNTZmNWNmMDJhMDZjYmRjOTM1MzAzZjJmNGFiZmNhMzQ5NmY3OWNkY2MwYjZkZWY3Y2QxMjc0YWJmNjQ2NWMzZjM1N2UxNGNlNDc4MTlhNDVmOTcyZTk1ZTI0ZjYxYWY0ZjUxZjFmMjg0ZDQwIn0=\",\"amount\":\"155000000000000\",\"gas\":\"155000000000000\"}}]},{\"receiver_id\":\"intents.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"mt_transfer_call\",\"arguments\":\"eyJyZWNlaXZlcl9pZCI6Iml6ZWMtaXNvbHVzZGMudGVtcGxhci1hbHBoYS5uZWFyIiwiYW1vdW50IjoiMTcyMTc2NyIsIm1zZyI6IlwiUmVwYXlcIiIsInRva2VuX2lkIjoibmVwMTQxOnNvbC01Y2UzYmYzYTMxYWYxOGJlNDBiYTMwZjcyMTEwMWI0MzQxNjkwMTg2Lm9tZnQubmVhciJ9\",\"amount\":\"1\",\"gas\":\"50000000000000\"}}]},{\"receiver_id\":\"izec-isolusdc.templar-alpha.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"withdraw_collateral\",\"arguments\":\"eyJhbW91bnQiOiI3MDUwMDAifQ==\",\"amount\":\"0\",\"gas\":\"50000000000000\"}}]}]}","signature":"ed25519:4CrFbqgiHo3BUv2QoHX7XDcRJqZeVvJJ8fB4SCgpzqSBPwsEyNCFT3uEKbwudDp2tzjQy9xPAeud6iq2qU1Crfet"}}"#)]
    #[case::passkey_old("b5a5dd68dfda.user0.tmplr.near", 174_191_790, 9, r#"{"Passkey":{"key":"p256:RNJ6vbZ93mRWA3spSrid6XPER7MPN1yAJ9TAZhR1hWPjHxFDXAqnsJLKA5y7yoU3PQ5yDkqkbrdmb2SPixsrzhhv","message":{"authenticator_data":"409c79ac0d851f51830ba9c308aa2729f751e57862ec267200376f1cafe07f121d00000000","client_data_json":"{\"type\":\"webauthn.get\",\"challenge\":\"hmW0QiGH3_vman3czsNtg_MkGC0se6bpbl4ysDzhdps\",\"origin\":\"https://app.templarfi.org\",\"crossOrigin\":false}","message":"{\"parameters\":{\"block_height\":\"174191790\",\"index\":\"0\",\"nonce\":\"9\"},\"account_id\":\"b5a5dd68dfda.user0.tmplr.near\",\"payload\":[{\"receiver_id\":\"pyth-oracle.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"update_price_feeds\",\"arguments\":\"eyJkYXRhIjoiNTA0ZTQxNTUwMTAwMDAwMDAzYjgwMTAwMDAwMDA0MGQwMDczMDBlNGIwM2U5YWZiYmRlMTkzZjU2YmIyZjZlMmEzZjgwY2Y0ZDcyY2JmMjc2NDhhNzc4NDZjM2MyYTkwOGI2NjYzY2Q3Mzk5OTkwNzU1YzI4ZDJlMDk5MjI3ZTA2Yzc5NzBiY2FlNGNlZGRlZjI2ODQxZmZjOTRlYjBjNWRjMDAwMjllZTlmMzcwMjUzZjdhNzNhOGM5M2ExNjY3ZWYzNGU1MzYwYWI1ZjU5ZDZiMGFiYjZjOWJhMTY0YTg2ZWNkZDMwYmU4N2M5NWYyMTNiNTVmY2RkZGQzZTg5M2IwNmI5ZWU5ZWQ3N2MyNGIxMDA2NGE3MmFmOGEyOTQyYjdiODFhMDEwMzgzOWM5ZmFmMzYxMGUzNzMzYWMxNDM1NDk4MTFkN2JhYTZiMDEzODk0OWYzMTJkMGQ5NWU2YTJiOTRmNGM2ZGQzMDk5N2ZlZWM0OTE2OTBlMDVmMThjMzY2YmU4MzA3NmZlMjI3ZWJjODIwN2YwNzI4M2NhYjQyOWI2ZDI0NzU5MDAwNGYwM2RkZDM2NWI4MDNlYTRmZTQ3YTU5OTkxMmExN2E1OGFlMGQwZjYzNTM5ZGY2NjIzZWFkZWFkY2EwNGU4YjQ0YWQwMzVmZWNmZjA1ZDhlN2RjMjFhMzQ3ZDliOTcyMTA3YzkxOTI1NDVhNzZkMzNkYWZkMzI3MDU5ZWY1ZGZkMDAwNjQzYzk0NGNiZDE1ZWVlNDZhYTY2YWI3NzE0NWExNmM4ODVhMzRhOTk4ZDdhZjNmNWFlMDdjOWVkMjRiYzY3YTE3OWY4YWFkOWVmNmYyNTRkNjFjNDIzNWI2OWE4OGY4NmIzZjlkMTdhNmE4YzczZjg4OTllNGFhODVmNjc0ZWE2MDEwODAwMWVjN2UyMDdmMjM1YjExMTI3Mjk4YTNmNDlkNjViMjIwZjkzZGRjNjA3ODFiMTg4NDM3Njg2ODQ0NzlkOWY0MzY1YzMyZmRmMGFhMzk3Mjc2NWFjYzljYjNhYjNlMjkzMzU4ZDkyMGYyNDg4OThhMGQ0M2E3ZDgwM2E1MWIxMDEwYjc1NTZlZTI3MzM3ZDM0MzU5YjFlZWE0OGVlMjE5NTJjNWRkN2I2NmQ4ZTlkZDMzMWJlNTBhZTFiZTgzYjViNmE1YzEwMDUwM2JiMTJlNzNiZjY2ZjYyM2VhZGExOGFhMGExMmU3NDUzYzc5ZjYyZDJiMjZiNmZjM2RhMGFjMmVlMDEwYzk0N2FlMmE3Y2I3ZDg5YmNhOWE1MzFkNWYwNDBjM2YzNGI0OTA4YjBmNmJkNjk5ZDMwOGNjYjRkZTkwOWZjMTkxNzhhZmMyYTE2MzNkN2NiYTYxZGExOTQyY2VhNDVjMzBlNjliMzE3NDg0M2RkNTI0MmIwZjIxMzBlMzM5M2RlMDAwZGFkYTI4OWQxM2NmMmU1ZGQ4MzE4MDAzNzgzOWJiZmE1ZGFhODAxNjcyYjViYTNiOTZkNjlhMTRiNGRlYzQzOGMyY2FlMmVlM2I2ZDQ2NTcxYzU0NzY3M2ViZTc1NTdkM2I0NmE0N2IxNGI2M2ZkM2NmNGYzZWQ2ZTE1NDA5MjgwMDAwZTcyYTIxMjczN2MzZWQyZjk3Mjg5MWM5OWIzNDNlMGQ1NjkwNDMxZDgwYjY2YmRlMzM5NTg0YWRlNzM3OTAwZTUxODhmMTVjZTNhOGE0OGYxZWMxMjMwN2ExNjI3YzYxMTlhMjhhYzU3MDVjNTdhNDI5OWRjMTJlYmJiYjlmMjQyMDAwZmE3M2RjMWQ2OWNlYTJjYzMxMzMyMDU2ZTY4MWRiN2UyZTQ4NjYzZjg4MmEwYTIwNGFhMWNmZjkyM2VmYmM0MTAwOWIxYTE3NTI0NzIzNjM5YWY0MjdiNTdmYjI4YWJhZDc3M2Q4OGJlZjE2M2M1N2Y2MjRjY2U4MzVkNTBmNDY5MDExMDJjMTcwMTViMDc4MDBiODg5ZTNjYjJlZjFiZWY0YTM5YThlNjRjZGViNWQ0YTEzM2UxN2M3ODVmNGMxOTIwN2ExNmYyZjA2MTA0ZjIyNWJjZmQ4ZDAyYjJjN2VlY2Y5YWFhYjQ2N2M5M2E2NWVkYWIwNTg0OGEwZDUwYWI1YzgxMDExMTJjZDMyMGM3YjQzOTljZWQ2YmQ2N2RmMWQyNjc5ODdjYjI2NTg0MTVkYmVjYjg1ZGRiODRmNzA0NjBlYjljNjM2YzZjMjU3ZDA1ZDU1OGZmZGJlNmMyOWJjYjFjYTZiOTViNTI1NmViYmZmYjRkMDkxYzhkODNmYmJiNGExMGRmMDE2OTI2OTVkYzAwMDAwMDAwMDAxYWUxMDFmYWVkYWM1ODUxZTMyYjliMjNiNWY5NDExYThjMmJhYzRhYWUzZWQ0ZGQ3YjgxMWRkMWE3MmVhNGFhNzEwMDAwMDAwMDBhNDc4ZjI5MDE0MTU1NTc1NjAwMDAwMDAwMDAwZjU1MzYwNzAwMDAyNzEwMzE0OGRjMDIxN2FkZTU5ZWUzNzUzZThiMzkxZDM1NjE5MTdmMzIyNDAyMDA1NTAwZWFhMDIwYzYxY2M0Nzk3MTI4MTM0NjFjZTE1Mzg5NGE5NmE2YzAwYjIxZWQwY2ZjMjc5OGQxZjlhOWU5Yzk0YTAwMDAwMDAwMDVmNTg2NWYwMDAwMDAwMDAwMDEzMjE5ZmZmZmZmZjgwMDAwMDAwMDY5MjY5NWRjMDAwMDAwMDA2OTI2OTVkYzAwMDAwMDAwMDVmNThiY2IwMDAwMDAwMDAwMDExNmY3MGQyN2M1ODE0OTFhNzI2NWNkNzllYThkN2Q0NThhNTFmYTY3NWRkNDlhOGY1NDIxZjdlOWI5MzhmZGUyNDdkM2FhMjgyZTZhMjViYzVjZjAwM2U4MTllNWM1NmNlMTBiNDZmMjkzMWE4Y2Y0YThjZGNmMmU3MDliNjkzYWQxOTI3NjI1MjZmOTVkMTgxNTM5OWFlOThmN2MyMTQ2YTViM2M5ZDM1MGI5YjUwNjJjOTYzYTRkNmFlMGFhNmExYjJlMmFiNzA3ZjRhZGE5NmQwYjhiMDJmYTlmNDhhOGZkNTMzOTJkMWY0Mjg3MTg0NjQ3YTkyNWNjYzEzMjY2NTBkM2ZmY2FkNDI5NTlhYTBhYzhlMjEwZjE1MDVhY2ZkNGNlMzhhNjRhODYxMmU5M2MxOTA1MjUyZGU5NWE2NzllY2MyZTNlN2RlYjNiNTQwMWM0ZmEzNjdmNzFhNDI3NzEzYzdhYTAwNDhiODcxYjJkZTM3NjBiZTM3Y2JmYzE4ODQ4Y2FlMWEyZmU0NGQ0MzM5MDRmMTk1NDRhMmM3ZDcwZmNmNjhiOTRlYmQ5MjMzOTBmNWJkNGUwZDYzMGVkMGE5YzE5ODAzOThmNGU5OWE5MzU1MTYxMjBhNWIyZDQxMzE2YjlkMmM0MmJiZWQ5NTI2MWRjZWMwYTZkNjdkNmRhNGZjZTJlMzVjN2I4MDA1NTAwYmU5YjU5ZDE3OGYwZDZhOTdhYjRjMzQzYmZmMmFhNjljYWExZWFhZTNlOTA0OGE2NTc4OGM1MjliMTI1YmIyNDAwMDAwMDBiZTg5YmUyYmUwMDAwMDAwMDAyZTFkYzM5ZmZmZmZmZjgwMDAwMDAwMDY5MjY5NWRjMDAwMDAwMDA2OTI2OTVkYzAwMDAwMDBiZWI0MDc3ZDgwMDAwMDAwMDAyZjBlY2RjMGRhNDc0MTlkMTEyYWI4ZmI5Mjc1YjU2YmVhYzcxODkzMDlhMmY0ZjhmM2I1NTczYmVlYTM0ZjMyNmZjNzk5NjBlZTg0MTc1NjUzOThlMTdlYWU0N2Q5OWJmY2RiMDA5NmY1MmYwYWE5NDgyMzhkMWYwMDVmYjVhOWQyMzkyZGJlYzdiMmNjMDRjMGFlMWVjZTQzZDgzODIyYzgyZjJkOTk0NWNmZWRmNzIwMjM1MDY0YjFhODQ5NTkxYTRkMjkyM2U0ZTAzZmFkYjFjODcyZDkzNTI3NWFiZmZmM2Q5ZWRjMWUyZGMzNWI1MDY2OWUxYjkxMDY0YmU4OTZlMDM3MDhhODhiMGUxZTZiN2NjM2MxZjQzOTBkY2FmMzA2NmY5MmQ3ZWIzOTRjMTUyODk2YmY1YzNhZTkzNjQ3ZjM1YmM5NDI4MTFmNTU5YzVjMDYxZmE5MzQxZjA1YzY0MzFhNDNmODY5MWJhMWUxYjJkZTM3NjBiZTM3Y2JmYzE4ODQ4Y2FlMWEyZmU0NGQ0MzM5MDRmMTk1NDRhMmM3ZDcwZmNmNjhiOTRlYmQ5MjMzOTBmNWJkNGUwZDYzMGVkMGE5YzE5ODAzOThmNGU5OWE5MzU1MTYxMjBhNWIyZDQxMzE2YjlkMmM0MmJiZWQ5NTI2MWRjZWMwYTZkNjdkNmRhNGZjZTJlMzVjN2I4In0=\",\"amount\":\"155000000000000\",\"gas\":\"155000000000000\"}}]},{\"receiver_id\":\"intents.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"mt_transfer_call\",\"arguments\":\"eyJyZWNlaXZlcl9pZCI6Iml6ZWMtaXNvbHVzZGMudjEudG1wbHIubmVhciIsImFtb3VudCI6IjI2ODk2NjIwOSIsIm1zZyI6IlwiUmVwYXlcIiIsInRva2VuX2lkIjoibmVwMTQxOnNvbC01Y2UzYmYzYTMxYWYxOGJlNDBiYTMwZjcyMTEwMWI0MzQxNjkwMTg2Lm9tZnQubmVhciJ9\",\"amount\":\"1\",\"gas\":\"50000000000000\"}}]},{\"receiver_id\":\"izec-isolusdc.v1.tmplr.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"withdraw_collateral\",\"arguments\":\"eyJhbW91bnQiOiI5NDAwMDAwMCJ9\",\"amount\":\"0\",\"gas\":\"50000000000000\"}}]}]}","signature":"MEYCIQCSJrl7L4RbU2JxR_6ues7FQKbYF6D95Wli_3sp9z9jmQIhAL7jHu_B4ZBjbAPHZuQxCnSnxIJsah_W5x2-3N3ncgEj"}}}"#)]
    #[case::passkey_flattened("b5a5dd68dfda.user0.tmplr.near", 174_191_790, 9, r#"{"Passkey":{"key":"p256:RNJ6vbZ93mRWA3spSrid6XPER7MPN1yAJ9TAZhR1hWPjHxFDXAqnsJLKA5y7yoU3PQ5yDkqkbrdmb2SPixsrzhhv","message":"{\"parameters\":{\"block_height\":\"174191790\",\"index\":\"0\",\"nonce\":\"9\"},\"account_id\":\"b5a5dd68dfda.user0.tmplr.near\",\"payload\":[{\"receiver_id\":\"pyth-oracle.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"update_price_feeds\",\"arguments\":\"eyJkYXRhIjoiNTA0ZTQxNTUwMTAwMDAwMDAzYjgwMTAwMDAwMDA0MGQwMDczMDBlNGIwM2U5YWZiYmRlMTkzZjU2YmIyZjZlMmEzZjgwY2Y0ZDcyY2JmMjc2NDhhNzc4NDZjM2MyYTkwOGI2NjYzY2Q3Mzk5OTkwNzU1YzI4ZDJlMDk5MjI3ZTA2Yzc5NzBiY2FlNGNlZGRlZjI2ODQxZmZjOTRlYjBjNWRjMDAwMjllZTlmMzcwMjUzZjdhNzNhOGM5M2ExNjY3ZWYzNGU1MzYwYWI1ZjU5ZDZiMGFiYjZjOWJhMTY0YTg2ZWNkZDMwYmU4N2M5NWYyMTNiNTVmY2RkZGQzZTg5M2IwNmI5ZWU5ZWQ3N2MyNGIxMDA2NGE3MmFmOGEyOTQyYjdiODFhMDEwMzgzOWM5ZmFmMzYxMGUzNzMzYWMxNDM1NDk4MTFkN2JhYTZiMDEzODk0OWYzMTJkMGQ5NWU2YTJiOTRmNGM2ZGQzMDk5N2ZlZWM0OTE2OTBlMDVmMThjMzY2YmU4MzA3NmZlMjI3ZWJjODIwN2YwNzI4M2NhYjQyOWI2ZDI0NzU5MDAwNGYwM2RkZDM2NWI4MDNlYTRmZTQ3YTU5OTkxMmExN2E1OGFlMGQwZjYzNTM5ZGY2NjIzZWFkZWFkY2EwNGU4YjQ0YWQwMzVmZWNmZjA1ZDhlN2RjMjFhMzQ3ZDliOTcyMTA3YzkxOTI1NDVhNzZkMzNkYWZkMzI3MDU5ZWY1ZGZkMDAwNjQzYzk0NGNiZDE1ZWVlNDZhYTY2YWI3NzE0NWExNmM4ODVhMzRhOTk4ZDdhZjNmNWFlMDdjOWVkMjRiYzY3YTE3OWY4YWFkOWVmNmYyNTRkNjFjNDIzNWI2OWE4OGY4NmIzZjlkMTdhNmE4YzczZjg4OTllNGFhODVmNjc0ZWE2MDEwODAwMWVjN2UyMDdmMjM1YjExMTI3Mjk4YTNmNDlkNjViMjIwZjkzZGRjNjA3ODFiMTg4NDM3Njg2ODQ0NzlkOWY0MzY1YzMyZmRmMGFhMzk3Mjc2NWFjYzljYjNhYjNlMjkzMzU4ZDkyMGYyNDg4OThhMGQ0M2E3ZDgwM2E1MWIxMDEwYjc1NTZlZTI3MzM3ZDM0MzU5YjFlZWE0OGVlMjE5NTJjNWRkN2I2NmQ4ZTlkZDMzMWJlNTBhZTFiZTgzYjViNmE1YzEwMDUwM2JiMTJlNzNiZjY2ZjYyM2VhZGExOGFhMGExMmU3NDUzYzc5ZjYyZDJiMjZiNmZjM2RhMGFjMmVlMDEwYzk0N2FlMmE3Y2I3ZDg5YmNhOWE1MzFkNWYwNDBjM2YzNGI0OTA4YjBmNmJkNjk5ZDMwOGNjYjRkZTkwOWZjMTkxNzhhZmMyYTE2MzNkN2NiYTYxZGExOTQyY2VhNDVjMzBlNjliMzE3NDg0M2RkNTI0MmIwZjIxMzBlMzM5M2RlMDAwZGFkYTI4OWQxM2NmMmU1ZGQ4MzE4MDAzNzgzOWJiZmE1ZGFhODAxNjcyYjViYTNiOTZkNjlhMTRiNGRlYzQzOGMyY2FlMmVlM2I2ZDQ2NTcxYzU0NzY3M2ViZTc1NTdkM2I0NmE0N2IxNGI2M2ZkM2NmNGYzZWQ2ZTE1NDA5MjgwMDAwZTcyYTIxMjczN2MzZWQyZjk3Mjg5MWM5OWIzNDNlMGQ1NjkwNDMxZDgwYjY2YmRlMzM5NTg0YWRlNzM3OTAwZTUxODhmMTVjZTNhOGE0OGYxZWMxMjMwN2ExNjI3YzYxMTlhMjhhYzU3MDVjNTdhNDI5OWRjMTJlYmJiYjlmMjQyMDAwZmE3M2RjMWQ2OWNlYTJjYzMxMzMyMDU2ZTY4MWRiN2UyZTQ4NjYzZjg4MmEwYTIwNGFhMWNmZjkyM2VmYmM0MTAwOWIxYTE3NTI0NzIzNjM5YWY0MjdiNTdmYjI4YWJhZDc3M2Q4OGJlZjE2M2M1N2Y2MjRjY2U4MzVkNTBmNDY5MDExMDJjMTcwMTViMDc4MDBiODg5ZTNjYjJlZjFiZWY0YTM5YThlNjRjZGViNWQ0YTEzM2UxN2M3ODVmNGMxOTIwN2ExNmYyZjA2MTA0ZjIyNWJjZmQ4ZDAyYjJjN2VlY2Y5YWFhYjQ2N2M5M2E2NWVkYWIwNTg0OGEwZDUwYWI1YzgxMDExMTJjZDMyMGM3YjQzOTljZWQ2YmQ2N2RmMWQyNjc5ODdjYjI2NTg0MTVkYmVjYjg1ZGRiODRmNzA0NjBlYjljNjM2YzZjMjU3ZDA1ZDU1OGZmZGJlNmMyOWJjYjFjYTZiOTViNTI1NmViYmZmYjRkMDkxYzhkODNmYmJiNGExMGRmMDE2OTI2OTVkYzAwMDAwMDAwMDAxYWUxMDFmYWVkYWM1ODUxZTMyYjliMjNiNWY5NDExYThjMmJhYzRhYWUzZWQ0ZGQ3YjgxMWRkMWE3MmVhNGFhNzEwMDAwMDAwMDBhNDc4ZjI5MDE0MTU1NTc1NjAwMDAwMDAwMDAwZjU1MzYwNzAwMDAyNzEwMzE0OGRjMDIxN2FkZTU5ZWUzNzUzZThiMzkxZDM1NjE5MTdmMzIyNDAyMDA1NTAwZWFhMDIwYzYxY2M0Nzk3MTI4MTM0NjFjZTE1Mzg5NGE5NmE2YzAwYjIxZWQwY2ZjMjc5OGQxZjlhOWU5Yzk0YTAwMDAwMDAwMDVmNTg2NWYwMDAwMDAwMDAwMDEzMjE5ZmZmZmZmZjgwMDAwMDAwMDY5MjY5NWRjMDAwMDAwMDA2OTI2OTVkYzAwMDAwMDAwMDVmNThiY2IwMDAwMDAwMDAwMDExNmY3MGQyN2M1ODE0OTFhNzI2NWNkNzllYThkN2Q0NThhNTFmYTY3NWRkNDlhOGY1NDIxZjdlOWI5MzhmZGUyNDdkM2FhMjgyZTZhMjViYzVjZjAwM2U4MTllNWM1NmNlMTBiNDZmMjkzMWE4Y2Y0YThjZGNmMmU3MDliNjkzYWQxOTI3NjI1MjZmOTVkMTgxNTM5OWFlOThmN2MyMTQ2YTViM2M5ZDM1MGI5YjUwNjJjOTYzYTRkNmFlMGFhNmExYjJlMmFiNzA3ZjRhZGE5NmQwYjhiMDJmYTlmNDhhOGZkNTMzOTJkMWY0Mjg3MTg0NjQ3YTkyNWNjYzEzMjY2NTBkM2ZmY2FkNDI5NTlhYTBhYzhlMjEwZjE1MDVhY2ZkNGNlMzhhNjRhODYxMmU5M2MxOTA1MjUyZGU5NWE2NzllY2MyZTNlN2RlYjNiNTQwMWM0ZmEzNjdmNzFhNDI3NzEzYzdhYTAwNDhiODcxYjJkZTM3NjBiZTM3Y2JmYzE4ODQ4Y2FlMWEyZmU0NGQ0MzM5MDRmMTk1NDRhMmM3ZDcwZmNmNjhiOTRlYmQ5MjMzOTBmNWJkNGUwZDYzMGVkMGE5YzE5ODAzOThmNGU5OWE5MzU1MTYxMjBhNWIyZDQxMzE2YjlkMmM0MmJiZWQ5NTI2MWRjZWMwYTZkNjdkNmRhNGZjZTJlMzVjN2I4MDA1NTAwYmU5YjU5ZDE3OGYwZDZhOTdhYjRjMzQzYmZmMmFhNjljYWExZWFhZTNlOTA0OGE2NTc4OGM1MjliMTI1YmIyNDAwMDAwMDBiZTg5YmUyYmUwMDAwMDAwMDAyZTFkYzM5ZmZmZmZmZjgwMDAwMDAwMDY5MjY5NWRjMDAwMDAwMDA2OTI2OTVkYzAwMDAwMDBiZWI0MDc3ZDgwMDAwMDAwMDAyZjBlY2RjMGRhNDc0MTlkMTEyYWI4ZmI5Mjc1YjU2YmVhYzcxODkzMDlhMmY0ZjhmM2I1NTczYmVlYTM0ZjMyNmZjNzk5NjBlZTg0MTc1NjUzOThlMTdlYWU0N2Q5OWJmY2RiMDA5NmY1MmYwYWE5NDgyMzhkMWYwMDVmYjVhOWQyMzkyZGJlYzdiMmNjMDRjMGFlMWVjZTQzZDgzODIyYzgyZjJkOTk0NWNmZWRmNzIwMjM1MDY0YjFhODQ5NTkxYTRkMjkyM2U0ZTAzZmFkYjFjODcyZDkzNTI3NWFiZmZmM2Q5ZWRjMWUyZGMzNWI1MDY2OWUxYjkxMDY0YmU4OTZlMDM3MDhhODhiMGUxZTZiN2NjM2MxZjQzOTBkY2FmMzA2NmY5MmQ3ZWIzOTRjMTUyODk2YmY1YzNhZTkzNjQ3ZjM1YmM5NDI4MTFmNTU5YzVjMDYxZmE5MzQxZjA1YzY0MzFhNDNmODY5MWJhMWUxYjJkZTM3NjBiZTM3Y2JmYzE4ODQ4Y2FlMWEyZmU0NGQ0MzM5MDRmMTk1NDRhMmM3ZDcwZmNmNjhiOTRlYmQ5MjMzOTBmNWJkNGUwZDYzMGVkMGE5YzE5ODAzOThmNGU5OWE5MzU1MTYxMjBhNWIyZDQxMzE2YjlkMmM0MmJiZWQ5NTI2MWRjZWMwYTZkNjdkNmRhNGZjZTJlMzVjN2I4In0=\",\"amount\":\"155000000000000\",\"gas\":\"155000000000000\"}}]},{\"receiver_id\":\"intents.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"mt_transfer_call\",\"arguments\":\"eyJyZWNlaXZlcl9pZCI6Iml6ZWMtaXNvbHVzZGMudjEudG1wbHIubmVhciIsImFtb3VudCI6IjI2ODk2NjIwOSIsIm1zZyI6IlwiUmVwYXlcIiIsInRva2VuX2lkIjoibmVwMTQxOnNvbC01Y2UzYmYzYTMxYWYxOGJlNDBiYTMwZjcyMTEwMWI0MzQxNjkwMTg2Lm9tZnQubmVhciJ9\",\"amount\":\"1\",\"gas\":\"50000000000000\"}}]},{\"receiver_id\":\"izec-isolusdc.v1.tmplr.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"withdraw_collateral\",\"arguments\":\"eyJhbW91bnQiOiI5NDAwMDAwMCJ9\",\"amount\":\"0\",\"gas\":\"50000000000000\"}}]}]}","signature":"MEYCIQCSJrl7L4RbU2JxR_6ues7FQKbYF6D95Wli_3sp9z9jmQIhAL7jHu_B4ZBjbAPHZuQxCnSnxIJsah_W5x2-3N3ncgEj","authenticator_data":"409c79ac0d851f51830ba9c308aa2729f751e57862ec267200376f1cafe07f121d00000000","client_data_json":"{\"type\":\"webauthn.get\",\"challenge\":\"hmW0QiGH3_vman3czsNtg_MkGC0se6bpbl4ysDzhdps\",\"origin\":\"https://app.templarfi.org\",\"crossOrigin\":false}"}}"#)]
    fn parse_all_formats_no_chain_id(
        #[case] verifying_account: AccountId,
        #[case] block_height: u64,
        #[case] nonce: u64,
        #[case] text: &str,
    ) {
        let json = serde_json::from_str::<ExecuteArgs<Box<[Transaction]>>>(text).unwrap();

        json.verify(
            &PayloadExecutionParameters::builder_empty()
                .verifying_contract(verifying_account)
                .with_key_parameters(KeyParameters {
                    block_height: block_height.into(),
                    index: 0.into(),
                    nonce: nonce.into(),
                })
                .build(),
            |o| o.is_none_or(|o| o == "https://app.templarfi.org"),
        )
        .unwrap();
    }

    #[rstest]
    #[case::passkey(r#"{"Passkey":{"key":"p256:R7J5Pp28zfnihdFnoTL1Ns7uLbow4LJbvmMzPg7MhKvroPthoVw88veNBpoyugC1zvpULndNtEBk9wBLytHnVau8","message":"{\"version\":\"1\",\"parameters\":{\"block_height\":\"12345\",\"index\":\"1\",\"nonce\":\"44\",\"name\":\"Templar Universal Account\",\"version\":\"1.2.1\",\"chain_id\":\"398\",\"verifying_contract\":\"my-universal-account.near\",\"salt\":\"2vkBPYckQRiVSZ6mEDWAtpK8k9thWMc6MwnKQTR1zFuq\"},\"payload\":[{\"receiver_id\":\"token.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"ft_transfer\",\"arguments\":\"eyJyZWNlaXZlcl9pZCI6InJlY2VpdmVyLm5lYXIiLCJhbW91bnQiOiIxMDAifQ==\",\"amount\":\"1\",\"gas\":\"30000000000000\"}}]}]}","signature":"MEUCIQDEZuomO2M0XhC_pRqXIZQH7Tv2x_IHEbjX7nluKB-66AIgYmah3iy9u5oudGc0qu8VLYM6p7AdUlqXYsrrkGpQF5E","authenticator_data":"0101010101010101010101010101010101010101010101010101010101010101","client_data_json":"{\"type\":\"type\",\"challenge\":\"cwNw1_U_XWfY9-P-7WQ3wyPUxzlUKdB3hVaTUjjxWGA\",\"origin\":\"origin\",\"crossOrigin\":null,\"topOrigin\":null}"}}"#)]
    #[case::ed25519_raw(r#"{"Ed25519Raw":{"key":"ed25519:8XxGb8AcgHB3xZhJ9q9mZpjwv1VYN3d5e9WPpQfzTwWT","message":"{\"version\":\"1\",\"parameters\":{\"block_height\":\"12345\",\"index\":\"1\",\"nonce\":\"44\",\"name\":\"Templar Universal Account\",\"version\":\"1.2.1\",\"chain_id\":\"398\",\"verifying_contract\":\"my-universal-account.near\",\"salt\":\"2vkBPYckQRiVSZ6mEDWAtpK8k9thWMc6MwnKQTR1zFuq\"},\"payload\":[{\"receiver_id\":\"token.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"ft_transfer\",\"arguments\":\"eyJyZWNlaXZlcl9pZCI6InJlY2VpdmVyLm5lYXIiLCJhbW91bnQiOiIxMDAifQ==\",\"amount\":\"1\",\"gas\":\"30000000000000\"}}]}]}","signature":"ed25519:5tR2qHLfP23vCVwfuFsZfFSjtUCV1CKXPyfjoEegbcpgri2cMohXHHaG5bZqGtcasnDHDLj5Btd6eGmnW4wRLS5h"}}"#)]
    #[case::sep53(r#"{"Sep53":{"key":"GBTSG6JQJN3FF443PWHWHRRSJOEX3INJ364HHI345S4HSUREQTHA5BRD","message":"{\"version\":\"1\",\"parameters\":{\"block_height\":\"12345\",\"index\":\"1\",\"nonce\":\"44\",\"name\":\"Templar Universal Account\",\"version\":\"1.2.1\",\"chain_id\":\"398\",\"verifying_contract\":\"my-universal-account.near\",\"salt\":\"2vkBPYckQRiVSZ6mEDWAtpK8k9thWMc6MwnKQTR1zFuq\"},\"payload\":[{\"receiver_id\":\"token.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"ft_transfer\",\"arguments\":\"eyJyZWNlaXZlcl9pZCI6InJlY2VpdmVyLm5lYXIiLCJhbW91bnQiOiIxMDAifQ==\",\"amount\":\"1\",\"gas\":\"30000000000000\"}}]}]}","signature":"ed25519:2XKrCfu8WufagnqSmMzycQugR453fq9nPGFeuxs4oVB8b3WJPRt8e6FrtKk1VvdMSsCVWNj29HGpwatUAhzM4E2U"}}"#)]
    #[case::eip191(r#"{"Eip191":{"key":"0x1494e3644415fcbb6ddc429e4caa1c885efef75b","message":"{\"version\":\"1\",\"parameters\":{\"block_height\":\"12345\",\"index\":\"1\",\"nonce\":\"44\",\"name\":\"Templar Universal Account\",\"version\":\"1.2.1\",\"chain_id\":\"398\",\"verifying_contract\":\"my-universal-account.near\",\"salt\":\"2vkBPYckQRiVSZ6mEDWAtpK8k9thWMc6MwnKQTR1zFuq\"},\"payload\":[{\"receiver_id\":\"token.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"ft_transfer\",\"arguments\":\"eyJyZWNlaXZlcl9pZCI6InJlY2VpdmVyLm5lYXIiLCJhbW91bnQiOiIxMDAifQ==\",\"amount\":\"1\",\"gas\":\"30000000000000\"}}]}]}","signature":{"r":"0x6ecfd6a8d1622a435da977a6c729c2520864547a59b18daf816a13f08e7f21d0","s":"0xf2122f2c7306bc963cdd855ab2401257e67434e55874698b6a4b7eb5bed7882","yParity":"0x1","v":"0x1"}}}"#)]
    fn parse_all_formats_with_chain_id(#[case] text: &str) {
        let json = serde_json::from_str::<ExecuteArgs<Box<[Transaction]>>>(text).unwrap();

        json.verify(
            &PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
                .verifying_contract(AccountId::from_str("my-universal-account.near").unwrap())
                .with_key_parameters(KeyParameters {
                    block_height: 12345.into(),
                    index: 1.into(),
                    nonce: 44.into(),
                })
                .build_salt(),
            |o| o.is_none_or(|o| o == "origin"),
        )
        .unwrap();
    }

    // These tests are the same cases as the previous test's, just with transpositions in the signatures, making them invalid.
    #[rstest]
    #[case::passkey(r#"{"Passkey":{"key":"p256:R7J5Pp28zfnihdFnoTL1Ns7uLbow4LJbvmMzPg7MhKvroPthoVw88veNBpoyugC1zvpULndNtEBk9wBLytHnVau8","message":"{\"version\":\"1\",\"parameters\":{\"block_height\":\"12345\",\"index\":\"1\",\"nonce\":\"44\",\"name\":\"Templar Universal Account\",\"version\":\"1.2.1\",\"chain_id\":\"398\",\"verifying_contract\":\"my-universal-account.near\",\"salt\":\"2vkBPYckQRiVSZ6mEDWAtpK8k9thWMc6MwnKQTR1zFuq\"},\"payload\":[{\"receiver_id\":\"token.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"ft_transfer\",\"arguments\":\"eyJyZWNlaXZlcl9pZCI6InJlY2VpdmVyLm5lYXIiLCJhbW91bnQiOiIxMDAifQ==\",\"amount\":\"1\",\"gas\":\"30000000000000\"}}]}]}","signature":"MEUCIQDEZuomO2M0XhC_pRqXIZQH7Tv2x_IHEbjX7nluKB-66AIgYmah3iy9u5oudGc0q8uVLYM6p7AdUlqXYsrrkGpQF5E","authenticator_data":"0101010101010101010101010101010101010101010101010101010101010101","client_data_json":"{\"type\":\"type\",\"challenge\":\"cwNw1_U_XWfY9-P-7WQ3wyPUxzlUKdB3hVaTUjjxWGA\",\"origin\":\"origin\",\"crossOrigin\":null,\"topOrigin\":null}"}}"#)]
    #[case::ed25519_raw(r#"{"Ed25519Raw":{"key":"ed25519:8XxGb8AcgHB3xZhJ9q9mZpjwv1VYN3d5e9WPpQfzTwWT","message":"{\"version\":\"1\",\"parameters\":{\"block_height\":\"12345\",\"index\":\"1\",\"nonce\":\"44\",\"name\":\"Templar Universal Account\",\"version\":\"1.2.1\",\"chain_id\":\"398\",\"verifying_contract\":\"my-universal-account.near\",\"salt\":\"2vkBPYckQRiVSZ6mEDWAtpK8k9thWMc6MwnKQTR1zFuq\"},\"payload\":[{\"receiver_id\":\"token.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"ft_transfer\",\"arguments\":\"eyJyZWNlaXZlcl9pZCI6InJlY2VpdmVyLm5lYXIiLCJhbW91bnQiOiIxMDAifQ==\",\"amount\":\"1\",\"gas\":\"30000000000000\"}}]}]}","signature":"ed25519:5tR2qHLfP23vCVfwuFsZfFSjtUCV1CKXPyfjoEegbcpgri2cMohXHHaG5bZqGtcasnDHDLj5Btd6eGmnW4wRLS5h"}}"#)]
    #[case::sep53(r#"{"Sep53":{"key":"GBTSG6JQJN3FF443PWHWHRRSJOEX3INJ364HHI345S4HSUREQTHA5BRD","message":"{\"version\":\"1\",\"parameters\":{\"block_height\":\"12345\",\"index\":\"1\",\"nonce\":\"44\",\"name\":\"Templar Universal Account\",\"version\":\"1.2.1\",\"chain_id\":\"398\",\"verifying_contract\":\"my-universal-account.near\",\"salt\":\"2vkBPYckQRiVSZ6mEDWAtpK8k9thWMc6MwnKQTR1zFuq\"},\"payload\":[{\"receiver_id\":\"token.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"ft_transfer\",\"arguments\":\"eyJyZWNlaXZlcl9pZCI6InJlY2VpdmVyLm5lYXIiLCJhbW91bnQiOiIxMDAifQ==\",\"amount\":\"1\",\"gas\":\"30000000000000\"}}]}]}","signature":"ed25519:2XKrCfu8WufagnqSmMzycQugR453fq9nPGFeuxs4oVB8b3WJPRt8e6FrtKk1VvdMSsCVWN2j9HGpwatUAhzM4E2U"}}"#)]
    #[case::eip191(r#"{"Eip191":{"key":"0x1494e3644415fcbb6ddc429e4caa1c885efef75b","message":"{\"version\":\"1\",\"parameters\":{\"block_height\":\"12345\",\"index\":\"1\",\"nonce\":\"44\",\"name\":\"Templar Universal Account\",\"version\":\"1.2.1\",\"chain_id\":\"398\",\"verifying_contract\":\"my-universal-account.near\",\"salt\":\"2vkBPYckQRiVSZ6mEDWAtpK8k9thWMc6MwnKQTR1zFuq\"},\"payload\":[{\"receiver_id\":\"token.near\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"ft_transfer\",\"arguments\":\"eyJyZWNlaXZlcl9pZCI6InJlY2VpdmVyLm5lYXIiLCJhbW91bnQiOiIxMDAifQ==\",\"amount\":\"1\",\"gas\":\"30000000000000\"}}]}]}","signature":{"r":"0x6ecfd6a8d1622a435da977a6c729c2520864547a59b18daf816a13f08e7f21d0","s":"0xf2122f2c7306bc963cdd85a5b2401257e67434e55874698b6a4b7eb5bed7882","yParity":"0x1","v":"0x1"}}}"#)]
    fn parse_all_formats_with_chain_id_invalid_signature(#[case] text: &str) {
        let json = serde_json::from_str::<ExecuteArgs<Box<[Transaction]>>>(text).unwrap();

        let e = json
            .verify(
                &PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
                    .verifying_contract(AccountId::from_str("my-universal-account.near").unwrap())
                    .with_key_parameters(KeyParameters {
                        block_height: 12345.into(),
                        index: 1.into(),
                        nonce: 44.into(),
                    })
                    .build_salt(),
                |o| o.is_none_or(|o| o == "origin"),
            )
            .unwrap_err();

        assert_eq!(
            e,
            VerificationError::Signature(CheckSignatureError::InvalidSignature),
        );
    }
}
