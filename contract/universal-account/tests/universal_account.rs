#![allow(clippy::unwrap_used)]

use near_sdk::{
    borsh, env,
    json_types::{U128, U64},
    serde_json::{self, json},
    NearToken,
};
use near_workspaces::{network::Sandbox, Worker};
use p256::{ecdsa::signature::Signer, elliptic_curve::rand_core::OsRng};
use rstest::rstest;
use templar_universal_account::{
    authentication::{
        ed25519::{eip191, raw, sep53},
        eip712,
        passkey::{
            self,
            data::{AuthenticatorData, ClientDataJson},
        },
        with_raw_string::WithRawString,
        HashForSigning, MessageWithSignature, Payload,
    },
    contract_state::Migration,
    transaction::{FunctionCallAction, Transaction},
    ExecuteArgs, ExecuteArgsMessage, KeyId, KeyParameters, PayloadExecutionParameters,
    NEAR_TESTNET_CHAIN_ID,
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

enum TestSigner {
    Passkey(p256::SecretKey),
    Ed25519Raw(ed25519_dalek::SigningKey),
    Eip712(alloy::signers::local::PrivateKeySigner),
    Sep53(ed25519_dalek::SigningKey),
    Eip191(alloy::signers::local::PrivateKeySigner),
}

impl TestSigner {
    fn random_passkey() -> Self {
        Self::Passkey(p256::SecretKey::random(&mut OsRng))
    }

    fn random_ed25519_raw() -> Self {
        Self::Ed25519Raw(ed25519_dalek::SigningKey::generate(&mut OsRng))
    }

    fn random_eip712() -> Self {
        Self::Eip712(alloy::signers::local::PrivateKeySigner::random())
    }

    fn random_sep53() -> Self {
        Self::Sep53(ed25519_dalek::SigningKey::generate(&mut OsRng))
    }

    fn random_eip191() -> Self {
        Self::Eip191(alloy::signers::local::PrivateKeySigner::random())
    }

    fn id(&self) -> KeyId {
        match self {
            Self::Passkey(key) => passkey::VerifyKey(key.public_key().into()).into(),
            Self::Ed25519Raw(key) => raw::VerifyKey(key.verifying_key().to_bytes().into()).into(),
            Self::Eip712(key) => eip712::VerifyKey(key.address().into()).into(),
            Self::Sep53(key) => sep53::VerifyKey(key.verifying_key().to_bytes().into()).into(),
            Self::Eip191(key) => eip191::VerifyKey(key.address().into()).into(),
        }
    }

    fn execute_args(
        &self,
        payload: WithRawString<Payload<Box<[Transaction]>>>,
    ) -> ExecuteArgs<Box<[Transaction]>> {
        match self {
            TestSigner::Passkey(secret_key) => {
                let payload = passkey::Message(payload);
                let challenge = payload.hash_for_signing();

                let message: MessageWithSignature<_> = payload.sign(
                    secret_key,
                    AuthenticatorData(Box::new([0xff_u8; 32])),
                    ClientDataJson {
                        r#type: "type".to_string(),
                        challenge: challenge.into(),
                        origin: "origin".to_string(),
                        cross_origin: None,
                        top_origin: None,
                    },
                );

                ExecuteArgsMessage {
                    key: passkey::VerifyKey(secret_key.public_key().into()),
                    mws: Box::new(message),
                }
                .into()
            }
            TestSigner::Ed25519Raw(key) => {
                let message = raw::Message::new(payload);
                let signature = key.sign(&message.preimage_for_signing()).to_bytes().into();
                let message = message.with_signature(signature);

                ExecuteArgsMessage {
                    key: raw::VerifyKey(key.verifying_key().to_bytes().into()),
                    mws: Box::new(message),
                }
                .into()
            }
            TestSigner::Eip712(key) => {
                let message = eip712::Message(payload);
                let mws = message.sign(key).unwrap();
                ExecuteArgsMessage {
                    key: eip712::VerifyKey(key.address().into()),
                    mws: Box::new(mws),
                }
                .into()
            }
            TestSigner::Sep53(key) => {
                let message = sep53::Message::new(payload);
                let signature = key.sign(&message.hash_for_signing()).to_bytes().into();
                let message = message.with_signature(signature);

                ExecuteArgsMessage {
                    key: sep53::VerifyKey(key.verifying_key().to_bytes().into()),
                    mws: Box::new(message),
                }
                .into()
            }
            TestSigner::Eip191(key) => {
                let message = eip191::Message(payload);
                let mws = message.sign(key).unwrap();
                ExecuteArgsMessage {
                    key: eip191::VerifyKey(key.address().into()),
                    mws: Box::new(mws),
                }
                .into()
            }
        }
    }
}

struct Setup {
    uac: UniversalAccountController,
    ft: FtController,
    third_party: near_workspaces::Account,
}

async fn setup(worker: &Worker<Sandbox>, sk: &TestSigner, migrated: bool) -> Setup {
    test_utils::accounts!(worker, uni_account, ft_account, third_party);

    let make_uac = || async move {
        if migrated {
            let c = uni_account
                .deploy(UniversalAccountController::wasm_0_2_0())
                .await
                .unwrap()
                .unwrap();
            c.call("new")
                .args_json(json!({
                    "key": sk.id(),
                }))
                .transact()
                .await
                .unwrap()
                .unwrap();

            let ua = uni_account
                .deploy(UniversalAccountController::wasm().await)
                .await
                .unwrap()
                .unwrap();

            let ua = UniversalAccountController { contract: ua };

            let r = ua
                .migrate(
                    ua.contract().as_account(),
                    Migration::V0 {
                        chain_id: U128(NEAR_TESTNET_CHAIN_ID),
                    },
                )
                .await;

            for o in r.outcomes() {
                o.clone().into_result().unwrap();
            }

            ua
        } else {
            UniversalAccountController::deploy(uni_account, sk.id(), NEAR_TESTNET_CHAIN_ID).await
        }
    };

    let (uac, ft) = tokio::join!(
        make_uac(),
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
    #[values(
        (TestSigner::random_passkey(), false),
        (TestSigner::random_passkey(), true),
        (TestSigner::random_ed25519_raw(), false),
        (TestSigner::random_ed25519_raw(), true),
        (TestSigner::random_eip712(), false),
        (TestSigner::random_sep53(), false),
        (TestSigner::random_eip191(), false),
    )]
    (sk, migrated): (TestSigner, bool),
) {
    let Setup {
        uac,
        ft,
        third_party,
    } = setup(&worker, &sk, migrated).await;

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

    let payload = WithRawString::from_parsed(Payload::new(
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .with_key_parameters(KeyParameters {
                block_height,
                index: U64(0),
                nonce: U64(1),
            })
            .verifying_contract(uac.contract().id().clone())
            .build_salt(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    ));

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
    assert_eq!(key_entry.chain_id, Some(NEAR_TESTNET_CHAIN_ID.into()));
    assert_eq!(key_entry.name, Some("Templar Universal Account".into()));
    assert_eq!(&key_entry.verifying_contract, uac.contract().id());
    assert_eq!(key_entry.version, Some("1.2.1".into()));
    assert_eq!(
        key_entry.salt,
        Some(
            env::keccak256_array(
                &borsh::to_vec(&(key_entry.block_height, key_entry.index)).unwrap()
            )
            .into()
        )
    );

    // Second execution, check nonce advancement

    let payload = WithRawString::from_parsed(Payload::new(
        key_entry.next_nonce(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    ));

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
#[should_panic = "Smart contract panicked: Execution parameter `nonce` mismatch: expected `2`, got `3`"]
async fn skip_nonce(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(
        (TestSigner::random_passkey(), false),
        (TestSigner::random_passkey(), true),
        (TestSigner::random_ed25519_raw(), false),
        (TestSigner::random_ed25519_raw(), true),
        (TestSigner::random_eip712(), false),
        (TestSigner::random_sep53(), false),
        (TestSigner::random_eip191(), false),
    )]
    (sk, migrated): (TestSigner, bool),
) {
    let Setup {
        uac,
        ft,
        third_party,
    } = setup(&worker, &sk, migrated).await;

    let key_entry = uac.get_key(sk.id()).await.unwrap();
    let block_height = key_entry.block_height;

    let payload = WithRawString::from_parsed(Payload::new(
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .with_key_parameters(KeyParameters {
                block_height,
                index: U64(0),
                nonce: U64(1),
            })
            .verifying_contract(uac.contract().id().clone())
            .build_salt(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    ));

    let execute_args = sk.execute_args(payload);

    uac.execute(&third_party, execute_args).await;

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should not succeed");

    let key_entry = uac.get_key(sk.id()).await.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 1);

    // Try to skip a nonce

    let payload = WithRawString::from_parsed(Payload::new(
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .with_key_parameters(KeyParameters {
                block_height,
                index: U64(0),
                nonce: U64(3),
            })
            .verifying_contract(uac.contract().id().clone())
            .build_salt(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    ));

    let execute_args = sk.execute_args(payload);

    uac.execute(&third_party, execute_args).await;
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Execution parameter `nonce` mismatch: expected `2`, got `1`"]
async fn reuse_nonce(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(
        (TestSigner::random_passkey(), false),
        (TestSigner::random_passkey(), true),
        (TestSigner::random_ed25519_raw(), false),
        (TestSigner::random_ed25519_raw(), true),
        (TestSigner::random_eip712(), false),
        (TestSigner::random_sep53(), false),
        (TestSigner::random_eip191(), false),
    )]
    (sk, migrated): (TestSigner, bool),
) {
    let Setup {
        uac,
        ft,
        third_party,
    } = setup(&worker, &sk, migrated).await;

    let key_entry = uac.get_key(sk.id()).await.unwrap();
    let block_height = key_entry.block_height;

    let payload = WithRawString::from_parsed(Payload::new(
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .with_key_parameters(KeyParameters {
                block_height,
                index: U64(0),
                nonce: U64(1),
            })
            .verifying_contract(uac.contract().id().clone())
            .build_salt(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    ));

    let execute_args = sk.execute_args(payload);

    uac.execute(&third_party, execute_args).await;

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should succeed");

    let key_entry = uac.get_key(sk.id()).await.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 1);

    // Try to reuse a nonce

    let payload = WithRawString::from_parsed(Payload::new(
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .with_key_parameters(KeyParameters {
                block_height,
                index: U64(0),
                nonce: U64(1),
            })
            .verifying_contract(uac.contract().id().clone())
            .build_salt(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    ));

    let execute_args = sk.execute_args(payload);

    uac.execute(&third_party, execute_args).await;
}
