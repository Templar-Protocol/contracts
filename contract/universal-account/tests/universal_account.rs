#![allow(clippy::unwrap_used)]

use near_sdk::{
    json_types::{U128, U64},
    serde_json::{self, json},
    NearToken,
};
use near_workspaces::{network::Sandbox, Worker};
use p256::{ecdsa::signature::Signer, elliptic_curve::rand_core::OsRng};
use rstest::rstest;
use templar_universal_account::{
    authentication::{
        ed25519_raw::{self, Ed25519RawKey},
        passkey::{
            self,
            data::{AuthenticatorData, ClientDataJson},
            Passkey,
        },
        with_raw_string::WithRawString,
        HashForSigning, Payload,
    },
    transaction::{FunctionCallAction, Transaction},
    ExecuteArgs, ExecutionParameters, KeyId,
};
use test_utils::{
    controller::universal_account::UniversalAccountController, worker, ContractController,
    FtController, StorageManagementController,
};

fn mint(amount: u128) -> FunctionCallAction {
    FunctionCallAction {
        function_name: "mint".to_string(),
        arguments: serde_json::to_vec(&json!({
            "amount": U128(amount),
        }))
        .unwrap()
        .into(),
        amount: NearToken::from_near(0),
        gas: near_sdk::Gas::from_tgas(30),
    }
}

enum Sk {
    Passkey(p256::SecretKey),
    Ed25519Raw(ed25519_dalek::SigningKey),
}

impl Sk {
    fn random_passkey() -> Self {
        Self::Passkey(p256::SecretKey::random(&mut OsRng))
    }

    fn random_ed25519_raw() -> Self {
        Self::Ed25519Raw(ed25519_dalek::SigningKey::generate(&mut OsRng))
    }

    fn id(&self) -> KeyId {
        match self {
            Sk::Passkey(secret_key) => KeyId::Passkey(Passkey(secret_key.public_key().into())),
            Sk::Ed25519Raw(signing_key) => {
                KeyId::Ed25519RawKey(Ed25519RawKey(signing_key.verifying_key().to_bytes().into()))
            }
        }
    }

    fn execute_args(&self, payload: WithRawString<Payload<Box<[Transaction]>>>) -> ExecuteArgs {
        match self {
            Sk::Passkey(secret_key) => {
                let payload = passkey::Message(payload);
                let challenge = payload.hash_for_signing();

                let message: passkey::MessageWithSignature<_> = payload
                    .sign(
                        secret_key,
                        AuthenticatorData(Box::new([0xff_u8; 32])),
                        ClientDataJson {
                            r#type: "type".to_string(),
                            challenge: challenge.into(),
                            origin: "origin".to_string(),
                            cross_origin: None,
                            top_origin: None,
                        },
                    )
                    .try_into()
                    .unwrap();

                ExecuteArgs::Passkey {
                    key: Passkey(secret_key.public_key().into()),
                    message: Box::new(message),
                }
            }
            Sk::Ed25519Raw(signing_key) => {
                let message = ed25519_raw::Message(payload);
                let signature = signing_key
                    .sign(&message.preimage_for_signing())
                    .to_bytes()
                    .into();
                let message = ed25519_raw::MessageWithSignature { message, signature };

                ExecuteArgs::Ed25519Raw {
                    key: Ed25519RawKey(signing_key.verifying_key().to_bytes().into()),
                    message: Box::new(message),
                }
            }
        }
    }
}

#[test]
fn ed_sign() {
    let key = ed25519_dalek::SigningKey::generate(&mut OsRng);
    let message = b"Hello";
    let signature = key.sign(message.as_slice()).to_bytes();
    let verify_result =
        near_sdk::env::ed25519_verify(&signature, message, key.verifying_key().as_bytes());
    eprintln!("Result: {verify_result:?}");
}

struct Setup {
    uac: UniversalAccountController,
    ft: FtController,
    third_party: near_workspaces::Account,
}

async fn setup(worker: &Worker<Sandbox>, sk: &Sk) -> Setup {
    test_utils::accounts!(worker, uni_account, ft_account, third_party);

    let (uac, ft) = tokio::join!(
        UniversalAccountController::deploy(uni_account, sk.id()),
        FtController::deploy(ft_account, "Fungible Token", "FT"),
    );

    ft.storage_deposit_for(
        &third_party,
        uac.contract().id(),
        NearToken::from_near(1).saturating_div(4),
    )
    .await;

    Setup {
        uac,
        ft,
        third_party,
    }
}

#[rstest]
#[tokio::test]
pub async fn universal_account(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(Sk::random_passkey(), Sk::random_ed25519_raw())] sk: Sk,
) {
    let Setup {
        uac,
        ft,
        third_party,
    } = setup(&worker, &sk).await;

    let key_list = uac.list_keys(None, None).await;
    assert_eq!(
        key_list,
        vec![sk.id()],
        "Key should be the only one in control of the account immediately after deployment"
    );

    let key_entry = uac.get_key(sk.id()).await.unwrap();
    let block_height = key_entry.block_height;

    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 0);

    let payload = WithRawString::from_parsed(Payload {
        parameters: ExecutionParameters {
            block_height,
            index: U64(0),
            nonce: U64(1),
        },
        account_id: uac.contract().id().clone(),
        payload: vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    });

    let execute_args = sk.execute_args(payload);

    let e = uac.execute(&third_party, execute_args).await;

    for o in e.outcomes() {
        assert!(o.is_success(), "Expect success on all receipts: {o:?}");
    }

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should succeed");

    let key_entry = uac.get_key(sk.id()).await.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 1);

    // Second execution, check nonce advancement

    let payload = WithRawString::from_parsed(Payload {
        parameters: ExecutionParameters {
            block_height,
            index: U64(0),
            nonce: U64(2),
        },
        account_id: uac.contract().id().clone(),
        payload: vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    });

    let execute_args = sk.execute_args(payload);

    let e = uac.execute(&third_party, execute_args).await;

    for o in e.outcomes() {
        assert!(o.is_success(), "Expect success on all receipts: {o:?}");
    }

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 200, "Function call should succeed");

    let key_entry = uac.get_key(sk.id()).await.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 2);
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Nonce mismatch"]
async fn skip_nonce(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(Sk::random_passkey(), Sk::random_ed25519_raw())] sk: Sk,
) {
    let Setup {
        uac,
        ft,
        third_party,
    } = setup(&worker, &sk).await;

    let key_entry = uac.get_key(sk.id()).await.unwrap();
    let block_height = key_entry.block_height;

    let payload = WithRawString::from_parsed(Payload {
        parameters: ExecutionParameters {
            block_height,
            index: U64(0),
            nonce: U64(1),
        },
        account_id: uac.contract().id().clone(),
        payload: vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    });

    let execute_args = sk.execute_args(payload);

    uac.execute(&third_party, execute_args).await;

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should not succeed");

    let key_entry = uac.get_key(sk.id()).await.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 1);

    // Try to skip a nonce

    let payload = WithRawString::from_parsed(Payload {
        parameters: ExecutionParameters {
            block_height,
            index: U64(0),
            nonce: U64(3),
        },
        account_id: uac.contract().id().clone(),
        payload: vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    });

    let execute_args = sk.execute_args(payload);

    uac.execute(&third_party, execute_args).await;
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Nonce mismatch"]
async fn reuse_nonce(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(Sk::random_passkey(), Sk::random_ed25519_raw())] sk: Sk,
) {
    let Setup {
        uac,
        ft,
        third_party,
    } = setup(&worker, &sk).await;

    let key_entry = uac.get_key(sk.id()).await.unwrap();
    let block_height = key_entry.block_height;

    let payload = WithRawString::from_parsed(Payload {
        parameters: ExecutionParameters {
            block_height,
            index: U64(0),
            nonce: U64(1),
        },
        account_id: uac.contract().id().clone(),
        payload: vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    });

    let execute_args = sk.execute_args(payload);

    uac.execute(&third_party, execute_args).await;

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should succeed");

    let key_entry = uac.get_key(sk.id()).await.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 1);

    // Try to reuse a nonce

    let payload = WithRawString::from_parsed(Payload {
        parameters: ExecutionParameters {
            block_height,
            index: U64(0),
            nonce: U64(1),
        },
        account_id: uac.contract().id().clone(),
        payload: vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    });

    let execute_args = sk.execute_args(payload);

    uac.execute(&third_party, execute_args).await;
}
