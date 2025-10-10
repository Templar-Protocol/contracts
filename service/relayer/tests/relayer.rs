#![allow(clippy::unwrap_used)]

use std::str::FromStr;

use axum::{extract::State, Json};
use clap::Parser;
use near_jsonrpc_client::methods::tx::TransactionInfo;
use near_primitives::{
    action::{
        delegate::{DelegateAction, SignedDelegateAction},
        Action, FunctionCallAction,
    },
    views::TxExecutionStatus,
};
use near_sdk::{json_types::U64, NearToken};
use near_workspaces::{network::Sandbox, Account, Worker};
use p256::elliptic_curve::rand_core::OsRng;
use rstest::{fixture, rstest};
use tokio::sync::watch;

use templar_common::registry::DeployMode;
use templar_relayer::{
    app::{App, Configuration},
    route::{
        relay::RelayRequest as SdaRelayRequest,
        universal_account::{
            create::{CreatePasskeyAccount, CreateRequest},
            pow::Pow,
            relay::RelayRequest as UaRelayRequest,
        },
        SimpleResponse,
    },
};
use templar_universal_account::{
    authentication::passkey::{
        self,
        data::{AuthenticatorData, ClientDataJson},
        with_raw_string::WithRawString,
        Passkey, Payload, UncheckedMessage,
    },
    encoding::p256::PublicKey,
    transaction::{self, Transaction},
    ExecuteArgs, ExecutionParameters,
};
use test_utils::{
    controller::universal_account::UniversalAccountController, setup_test_w, ContractController,
    RegistryController, UnifiedMarketController,
};

const POW_DIFFICULTY: usize = 6;

struct InitTest {
    worker: Worker<Sandbox>,
    app: App,
    c: UnifiedMarketController,
    ua_deployer: RegistryController,
    borrow_user: Account,
    relay_user: Account,
}

#[fixture]
async fn init_test() -> InitTest {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let worker = near_workspaces::sandbox_with_version("2.7.0")
        .await
        .unwrap();
    setup_test_w!(worker extract(c) accounts(borrow_user, relay_user, ua_deployer));
    let rpc_addr = worker.rpc_addr();

    let ua_deployer = RegistryController::new(ua_deployer).await;
    ua_deployer
        .add_version(
            ua_deployer.contract().as_account(),
            NearToken::from_near(40),
            "v1",
            DeployMode::GlobalHash,
            UniversalAccountController::wasm().await,
        )
        .await;

    let kill = watch::Sender::default();
    let mut app = App::new(
        Configuration::parse_from([
            "relayer",
            "--rpc-url",
            &rpc_addr,
            "--database-url",
            "postgres://relayeruser:password@0.0.0.0:5432/relayer",
            "--monitor-market",
            c.market.contract().id().as_ref(),
            "--relay-account-id",
            relay_user.id().as_ref(),
            "--relay-secret-key",
            &relay_user.secret_key().to_string(),
            "--ua-account-id",
            ua_deployer.contract().id().as_ref(),
            "--ua-secret-key",
            &ua_deployer.contract().as_account().secret_key().to_string(),
            "--ua-pow-difficulty",
            &POW_DIFFICULTY.to_string(),
            "--ua-registry-id",
            ua_deployer.contract().id().as_ref(),
            "--ua-version-key",
            "v1",
        ]),
        kill,
    );
    app.database.migrate().await.unwrap();
    app.load_markets().await;

    InitTest {
        worker,
        app,
        c,
        ua_deployer,
        borrow_user,
        relay_user,
    }
}

#[rstest]
#[tokio::test]
pub async fn delegate_action(#[future(awt)] init_test: InitTest) {
    let InitTest {
        worker,
        app,
        c,
        borrow_user,
        relay_user,
        ..
    } = init_test;

    // Relay a signed delegate action.

    let fetch_nonce = app
        .relay_near
        .fetch_nonce(
            borrow_user.id().clone(),
            borrow_user.secret_key().public_key().into(),
        )
        .await
        .unwrap();

    let delegate_action = DelegateAction {
        sender_id: borrow_user.id().clone(),
        receiver_id: c.market.contract().id().clone(),
        actions: vec![Action::from(FunctionCallAction {
            method_name: "apply_interest".to_string(),
            args: b"{}".to_vec(),
            gas: 30 * 10_u64.pow(12),
            deposit: 0,
        })
        .try_into()
        .unwrap()],
        nonce: fetch_nonce.nonce + 1,
        max_block_height: fetch_nonce.block_height + 360,
        public_key: borrow_user.secret_key().public_key().into(),
    };

    let signature = near_crypto::SecretKey::from_str(&borrow_user.secret_key().to_string())
        .unwrap()
        .sign(&delegate_action.get_nep461_hash().0);

    let signed_delegate_action = SignedDelegateAction {
        delegate_action,
        signature,
    };

    let response = templar_relayer::route::relay::relay(
        State(app.clone()),
        Json(SdaRelayRequest {
            signed_delegate_action,
            storage_deposit: false,
            wait_until: TxExecutionStatus::Final,
        }),
    )
    .await;

    let SimpleResponse::Success(response) = response else {
        panic!("Relay attempt should succeed");
    };

    let status = worker
        .tx_status(
            TransactionInfo::TransactionId {
                tx_hash: response.transaction_hash,
                sender_account_id: relay_user.id().clone(),
            },
            TxExecutionStatus::Final,
        )
        .await
        .unwrap();

    status
        .final_execution_outcome
        .unwrap()
        .into_outcome()
        .assert_success();
}

#[rstest]
#[tokio::test]
pub async fn universal_account(#[future(awt)] init_test: InitTest) {
    let InitTest {
        worker,
        app,
        c,
        ua_deployer,
        borrow_user,
        ..
    } = init_test;

    // Relay a signed delegate action.

    let fetch_nonce = app
        .relay_near
        .fetch_nonce(
            borrow_user.id().clone(),
            borrow_user.secret_key().public_key().into(),
        )
        .await
        .unwrap();

    // Deploy a universal account.

    let secret_key = p256::SecretKey::random(&mut OsRng);
    let passkey = Passkey(PublicKey(secret_key.public_key()));

    let payload = WithRawString::from_parsed(Payload {
        parameters: ExecutionParameters {
            index: U64(0),
            nonce: U64(0),
        },
        account_id: ua_deployer.contract().id().clone(),
        payload: Pow::mine(
            CreatePasskeyAccount {
                key: passkey.clone(),
                block_hash: fetch_nonce.block_hash,
            },
            POW_DIFFICULTY,
            10_000,
        )
        .unwrap(),
    });

    let challenge = payload.hash().into();

    let message: passkey::Message<_> = passkey::UncheckedMessage::new_and_sign(
        &secret_key,
        payload,
        AuthenticatorData(Box::new([0xffu8; 32])),
        WithRawString::from_parsed(ClientDataJson {
            r#type: "type".to_string(),
            challenge,
            origin: "origin".to_string(),
            cross_origin: None,
            top_origin: None,
        }),
    )
    .try_into()
    .unwrap();

    let response = templar_relayer::route::universal_account::create::create(
        State(app.clone()),
        Json(CreateRequest::Passkey(message)),
    )
    .await;

    eprintln!("UA deploy response: {response:?}");

    let SimpleResponse::Success(response) = response else {
        panic!("Universal account deployment should succeed");
    };

    let ua_account_id = response.account_id.clone();

    let status = worker
        .tx_status(
            TransactionInfo::TransactionId {
                tx_hash: response.transaction_hash,
                sender_account_id: ua_deployer.contract().id().clone(),
            },
            TxExecutionStatus::Final,
        )
        .await
        .unwrap();

    eprintln!("UA deploy status: {status:?}");

    status
        .final_execution_outcome
        .unwrap()
        .into_outcome()
        .assert_success();

    // Send an action to the universal account contract

    let payload = WithRawString::from_parsed(Payload {
        parameters: ExecutionParameters {
            index: 0.into(),
            nonce: 1.into(),
        },
        account_id: ua_account_id.clone(),
        payload: vec![Transaction {
            receiver_id: c.contract().id().clone(),
            actions: Box::new([transaction::Action::FunctionCall {
                function_name: "apply_interest".to_string(),
                arguments: b"{}".to_vec().into(),
                amount: NearToken::from_near(0),
                gas: near_sdk::Gas::from_tgas(250),
            }]),
        }]
        .into(),
    });
    let challenge = payload.hash().into();
    let message: passkey::Message<_> = UncheckedMessage::new_and_sign(
        &secret_key,
        payload,
        AuthenticatorData(Box::new([0xff_u8; 32])),
        WithRawString::from_parsed(ClientDataJson {
            r#type: "type".to_string(),
            challenge,
            origin: "origin".to_string(),
            cross_origin: None,
            top_origin: None,
        }),
    )
    .try_into()
    .unwrap();

    let response = templar_relayer::route::universal_account::relay::relay(
        State(app.clone()),
        Json(UaRelayRequest {
            account_id: ua_account_id.clone(),
            args: ExecuteArgs::Passkey {
                key: passkey,
                message,
            },
        }),
    )
    .await;

    eprintln!("UA Relay response: {response:?}");

    let response = match response {
        SimpleResponse::Success(response) => response,
        e => {
            panic!("Should succeed: {e:?}");
        }
    };

    let status = worker
        .tx_status(
            TransactionInfo::TransactionId {
                tx_hash: response.transaction_hash,
                sender_account_id: ua_account_id.clone(),
            },
            TxExecutionStatus::Final,
        )
        .await
        .unwrap();

    eprintln!("UA relay status: {status:?}");

    status
        .final_execution_outcome
        .unwrap()
        .into_outcome()
        .assert_success();
}
