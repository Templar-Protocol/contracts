#![allow(clippy::unwrap_used)]

use near_sdk::{
    json_types::{U128, U64},
    serde_json::{self, json},
    NearToken,
};
use near_workspaces::{network::Sandbox, Worker};
use p256::elliptic_curve::rand_core::OsRng;
use rstest::rstest;
use templar_universal_account::{
    authentication::passkey::{
        self,
        data::{AuthenticatorData, ClientDataJson},
        with_raw_string::WithRawString,
        Passkey, Payload, UncheckedMessage,
    },
    encoding::p256::PublicKey,
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

fn make_message(
    secret_key: &p256::SecretKey,
    payload: WithRawString<Payload<Box<[Transaction]>>>,
) -> passkey::Message<Box<[Transaction]>> {
    let challenge = payload.hash();

    let message: passkey::Message<_> = UncheckedMessage::new_and_sign(
        secret_key,
        payload,
        AuthenticatorData(Box::new([0xff_u8; 32])),
        WithRawString::from_parsed(ClientDataJson {
            r#type: "type".to_string(),
            challenge: challenge.into(),
            origin: "origin".to_string(),
            cross_origin: None,
            top_origin: None,
        }),
    )
    .try_into()
    .unwrap();

    message
}

struct Setup {
    secret_key: p256::SecretKey,
    public_key: PublicKey,
    key_id: KeyId,
    uac: UniversalAccountController,
    ft: FtController,
    third_party: near_workspaces::Account,
}

#[rstest::fixture]
async fn setup(#[future(awt)] worker: Worker<Sandbox>) -> Setup {
    test_utils::accounts!(worker, uni_account, ft_account, third_party);

    let secret_key = p256::SecretKey::random(&mut OsRng);
    let public_key: PublicKey = secret_key.public_key().into();
    let key_id = KeyId::Passkey(Passkey(public_key.clone()));

    let (uac, ft) = tokio::join!(
        UniversalAccountController::deploy(uni_account, key_id.clone()),
        FtController::deploy(ft_account, "Fungible Token", "FT"),
    );

    ft.storage_deposit_for(
        &third_party,
        uac.contract().id(),
        NearToken::from_near(1).saturating_div(4),
    )
    .await;

    Setup {
        secret_key,
        public_key,
        key_id,
        uac,
        ft,
        third_party,
    }
}

#[rstest]
#[tokio::test]
pub async fn universal_account(#[future(awt)] setup: Setup) {
    let Setup {
        secret_key,
        public_key,
        key_id,
        uac,
        ft,
        third_party,
    } = setup;

    let key_list = uac.list_keys(None, None).await;
    assert_eq!(
        key_list,
        vec![key_id.clone()],
        "Key should be the only one in control of the account immediately after deployment"
    );

    let key_entry = uac.get_key(key_id.clone()).await.unwrap();
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

    let message = make_message(&secret_key, payload);

    let e = uac
        .execute(
            &third_party,
            ExecuteArgs::Passkey {
                key: Passkey(public_key.clone()),
                message,
            },
        )
        .await;

    for o in e.outcomes() {
        assert!(o.is_success(), "Expect success on all receipts: {o:?}");
    }

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should succeed");

    let key_entry = uac.get_key(key_id.clone()).await.unwrap();

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

    let message = make_message(&secret_key, payload);

    let e = uac
        .execute(
            &third_party,
            ExecuteArgs::Passkey {
                key: Passkey(public_key.clone()),
                message,
            },
        )
        .await;

    for o in e.outcomes() {
        assert!(o.is_success(), "Expect success on all receipts: {o:?}");
    }

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 200, "Function call should succeed");

    let key_entry = uac.get_key(key_id.clone()).await.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 2);
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Nonce mismatch"]
async fn skip_nonce(#[future(awt)] setup: Setup) {
    let Setup {
        secret_key,
        public_key,
        key_id,
        uac,
        ft,
        third_party,
    } = setup;

    let key_entry = uac.get_key(key_id.clone()).await.unwrap();
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

    let message = make_message(&secret_key, payload);

    uac.execute(
        &third_party,
        ExecuteArgs::Passkey {
            key: Passkey(public_key.clone()),
            message,
        },
    )
    .await;

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should not succeed");

    let key_entry = uac.get_key(key_id.clone()).await.unwrap();

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

    let message = make_message(&secret_key, payload);

    uac.execute(
        &third_party,
        ExecuteArgs::Passkey {
            key: Passkey(public_key.clone()),
            message,
        },
    )
    .await;
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Nonce mismatch"]
async fn reuse_nonce(#[future(awt)] setup: Setup) {
    let Setup {
        secret_key,
        public_key,
        key_id,
        uac,
        ft,
        third_party,
    } = setup;

    let key_entry = uac.get_key(key_id.clone()).await.unwrap();
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

    let message = make_message(&secret_key, payload);

    uac.execute(
        &third_party,
        ExecuteArgs::Passkey {
            key: Passkey(public_key.clone()),
            message,
        },
    )
    .await;

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should succeed");

    let key_entry = uac.get_key(key_id.clone()).await.unwrap();

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

    let message = make_message(&secret_key, payload);

    uac.execute(
        &third_party,
        ExecuteArgs::Passkey {
            key: Passkey(public_key.clone()),
            message,
        },
    )
    .await;
}
