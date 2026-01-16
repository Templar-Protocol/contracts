#![allow(clippy::unwrap_used)]

use std::{collections::HashSet, str::FromStr, time::Duration};

use axum::{extract::State, Json};
use clap::Parser;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::{
        delegate::{DelegateAction, SignedDelegateAction},
        Action, FunctionCallAction,
    },
    hash::CryptoHash,
    views::TxExecutionStatus,
};
use near_sdk::{
    env::sha256_array,
    json_types::Base64VecU8,
    serde_json::{self, json},
    AccountId, NearToken,
};
use p256::{
    ecdsa::{signature::Signer, SigningKey},
    elliptic_curve::rand_core::OsRng,
};
use rstest::{fixture, rstest};
use tokio::sync::watch;

use templar_common::{oracle::pyth::PriceIdentifier, registry::DeployMode};
use templar_relayer::{
    app::{args, App, Configuration},
    cache::Cache,
    client::{near::Near, pyth::Pyth},
    route::{
        relay::RelayRequest as SdaRelayRequest,
        universal_account::{
            create::{CreateRequest, CreateUniversalAccount},
            pow::Pow,
            relay::RelayRequest as UaRelayRequest,
        },
        SimpleResponse,
    },
};
use templar_universal_account::{
    authentication::{
        passkey::{
            self,
            data::{AuthenticatorData, ClientDataJson},
            Passkey,
        },
        HashForSigning, MessageWithSignature, Payload,
    },
    encoding::p256::PublicKey,
    transaction::{self, Transaction},
    ExecuteArgsMessage, KeyId, PayloadExecutionParameters, NEAR_TESTNET_CHAIN_ID,
};
use test_utils::*;

const POW_DIFFICULTY: usize = 6;

struct InitTest {
    worker: Sandbox,
    app: App,
    c: UnifiedMarketController,
    ua_deployer: RegistryController,
    borrow_user: TestAccount,
    relay_user: TestAccount,
}

fn create_message<T: near_sdk::serde::Serialize>(
    secret_key: &p256::SecretKey,
    parameters: PayloadExecutionParameters,
    payload: T,
) -> MessageWithSignature<passkey::Message<T>> {
    let payload = passkey::Message::from_parsed(Payload::new(parameters, payload));

    let challenge = payload.hash_for_signing().into();

    payload.sign(
        secret_key,
        AuthenticatorData(Box::new([0xffu8; 32])),
        ClientDataJson {
            r#type: "type".to_string(),
            challenge,
            origin: "origin".to_string(),
            cross_origin: None,
            top_origin: None,
        },
    )
}

fn create_execute_message(
    secret_key: &p256::SecretKey,
    parameters: PayloadExecutionParameters,
    receiver_id: AccountId,
    actions: impl Into<Box<[transaction::Action]>>,
) -> MessageWithSignature<passkey::Message<Box<[Transaction]>>> {
    create_message(
        secret_key,
        parameters,
        vec![Transaction {
            receiver_id,
            actions: actions.into(),
        }]
        .into_boxed_slice(),
    )
}

#[fixture]
async fn init_test(#[future(awt)] worker: Sandbox) -> InitTest {
    eprintln!("init_test");
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .try_init();

    eprintln!("setup_test");
    setup_test!(worker extract(c) accounts(borrow_user, relay_user, ua_deployer));

    eprintln!("ua_deployer");
    let ua_deployer = RegistryController::new(ua_deployer).await;
    eprintln!("add_version");
    ua_deployer
        .add_version(
            ua_deployer.account(),
            NearToken::from_near(80),
            "latest",
            DeployMode::GlobalHash,
            UniversalAccountController::wasm().await,
        )
        .await;

    let kill = watch::Sender::default();
    let mut app = App::new(
        Configuration::parse_from([
            "relayer",
            "--rpc-url",
            &worker.rpc_addr,
            "--database-url",
            "postgres://relayeruser:password@0.0.0.0:5432/relayer",
            "--monitor-market-id",
            c.market.account().id().as_ref(),
            "--relay-account-id",
            relay_user.id().as_ref(),
            "--relay-secret-key",
            &relay_user.secret_key.to_string(),
            "--ua-account-id",
            ua_deployer.account().id().as_ref(),
            "--ua-secret-key",
            &ua_deployer.account().secret_key.to_string(),
            "--ua-pow-difficulty",
            &POW_DIFFICULTY.to_string(),
            "--ua-registry-id",
            ua_deployer.account().id().as_ref(),
            "--ua-version-key",
            "latest",
            "--ua-chain-id",
            &NEAR_TESTNET_CHAIN_ID.to_string(),
            "--intents-id",
            "intents.near",
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

async fn poll_tx_status(_transaction_hash: CryptoHash, _sender_account_id: AccountId) {
    // TODO: Remove when near_api supports transaction status polling.
    tokio::time::sleep(Duration::from_secs(1)).await;

    // let status = worker
    //     .tx_status(
    //         TransactionInfo::TransactionId {
    //             tx_hash: response.transaction_hash,
    //             sender_account_id: ua_deployer.account().id().clone(),
    //         },
    //         TxExecutionStatus::Final,
    //     )
    //     .await
    //     .unwrap();

    // eprintln!("UA deploy status: {status:?}");

    // status
    //     .final_execution_outcome
    //     .unwrap()
    //     .into_outcome()
    //     .assert_success();
}

#[rstest]
#[tokio::test]
pub async fn delegate_action(#[future(awt)] init_test: InitTest) {
    let InitTest {
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
            borrow_user.public_key_string().parse().unwrap(),
        )
        .await
        .unwrap();

    let delegate_action = DelegateAction {
        sender_id: borrow_user.id().clone(),
        receiver_id: c.market.account().id().clone(),
        actions: vec![Action::from(FunctionCallAction {
            method_name: "apply_interest".to_string(),
            args: b"{}".to_vec(),
            gas: near_primitives::gas::Gas::from_teragas(30),
            deposit: NearToken::ZERO,
        })
        .try_into()
        .unwrap()],
        nonce: fetch_nonce.nonce + 1,
        max_block_height: fetch_nonce.block_height + 360,
        public_key: borrow_user.public_key_string().parse().unwrap(),
    };

    let signature = near_crypto::SecretKey::from_str(&borrow_user.secret_key.to_string())
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

    eprintln!("Transaction hash: {}", response.transaction_hash);

    poll_tx_status(response.transaction_hash, relay_user.id().clone()).await;
}

#[rstest]
#[tokio::test]
pub async fn universal_account_regression_0_2_0(#[future(awt)] init_test: InitTest) {
    let InitTest { worker, app, c, .. } = init_test;
    accounts!(worker, universal_account);

    let secret_key = p256::SecretKey::from_bytes(&[0xa8; 32].into()).unwrap();
    let passkey = Passkey(PublicKey(secret_key.public_key()));

    universal_account
        .deploy_init(
            UniversalAccountController::wasm_0_2_0().to_vec(),
            "new",
            json!({ "key": KeyId::Passkey(passkey.clone()) }),
        )
        .await;

    let parameters = app
        .ua_near
        .load_ua_key(
            universal_account.id().clone(),
            KeyId::Passkey(passkey.clone()),
        )
        .await
        .unwrap()
        .unwrap();

    app.database
        .create_account(
            universal_account.id(),
            NearToken::from_near(1).saturating_div(4),
        )
        .await
        .unwrap();

    let message = serde_json::to_string(&json!({
        "parameters": {
            "block_height": parameters.block_height,
            "index": "0",
            "nonce": "1",
        },
        "account_id": universal_account.id(),
        "payload": [{
            "receiver_id": c.market.account().id(),
            "actions": [{ "FunctionCall": {
                "function_name": "apply_interest",
                "arguments": Base64VecU8(b"{}".to_vec()),
                "amount": "0",
                "gas": "155000000000000",
            }}],
        }],
    }))
    .unwrap();

    let challenge = sha256_array(
        [
            b"\x19UAccount Signed Message:\n".to_vec(),
            message.as_bytes().to_vec(),
        ]
        .concat(),
    );

    let client_data_json = serde_json::to_string(&ClientDataJson {
        r#type: "webauthn.get".to_string(),
        challenge: passkey::data::Challenge(challenge),
        origin: "https://app.templarfi.org".to_string(),
        cross_origin: Some(false),
        top_origin: None,
    })
    .unwrap();

    let authenticator_data = AuthenticatorData(Box::new([0xff_u8; 32]));

    let sig_base = [
        &*authenticator_data,
        &near_sdk::env::sha256(client_data_json.as_bytes()),
    ]
    .concat();

    let signature = passkey::signature::Signature(SigningKey::from(secret_key).sign(&sig_base));

    let args_json = json!({
        "Passkey": {
            "key": passkey,
            "message": {
                "authenticator_data": authenticator_data,
                "client_data_json": client_data_json,
                "message": message,
                "signature": signature,
            }
        }
    });

    let args = serde_json::to_string(&args_json).unwrap();

    let response = templar_relayer::route::universal_account::relay::relay(
        State(app.clone()),
        Json(UaRelayRequest {
            account_id: universal_account.id().clone(),
            args: serde_json::from_str(&args).unwrap(),
            storage_deposit: HashSet::default(),
            update_price_feeds: HashSet::default(),
        }),
    )
    .await;

    let response = match response {
        SimpleResponse::Success(response) => response,
        e => {
            panic!("Should succeed: {e:?}");
        }
    };

    eprintln!("Transaction hash: {}", response.transaction_hash);

    poll_tx_status(response.transaction_hash, universal_account.id().clone()).await;
}

#[rstest]
#[tokio::test]
pub async fn universal_account(#[future(awt)] init_test: InitTest) {
    eprintln!("universal_account");
    let InitTest {
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
            borrow_user.public_key_string().parse().unwrap(),
        )
        .await
        .unwrap();

    // Deploy a universal account.

    let secret_key = p256::SecretKey::random(&mut OsRng);
    let passkey = Passkey(PublicKey(secret_key.public_key()));

    let message = create_message(
        &secret_key,
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .zero()
            .verifying_contract(ua_deployer.account().id().clone())
            .build_salt(),
        Pow::mine(
            CreateUniversalAccount {
                key: passkey.clone().into(),
                block_hash: fetch_nonce.block_hash,
            },
            POW_DIFFICULTY,
            10_000,
        )
        .unwrap(),
    );

    let response = templar_relayer::route::universal_account::create::create(
        State(app.clone()),
        Json(CreateRequest::ExecuteArgs(
            ExecuteArgsMessage {
                key: passkey.clone(),
                mws: Box::new(message),
            }
            .into(),
        )),
    )
    .await;

    eprintln!("UA deploy response: {response:?}");

    let SimpleResponse::Success(response) = response else {
        panic!("Universal account deployment should succeed");
    };

    let ua_account_id = response.account_id.clone();

    poll_tx_status(response.transaction_hash, ua_deployer.account.id.clone()).await;

    // Send an action to the universal account contract

    let load_parameters = async |account_id: AccountId, key: KeyId| {
        app.ua_near
            .load_ua_key(account_id, key)
            .await
            .unwrap()
            .unwrap()
    };

    let parameters = load_parameters(ua_account_id.clone(), KeyId::Passkey(passkey.clone())).await;

    let message = create_execute_message(
        &secret_key,
        parameters.next_nonce(),
        c.account().id().clone(),
        vec![transaction::FunctionCallAction {
            function_name: "apply_interest".to_string(),
            arguments: b"{}".to_vec().into(),
            amount: NearToken::ZERO,
            gas: near_sdk::Gas::from_tgas(250),
        }
        .into()],
    );

    let response = templar_relayer::route::universal_account::relay::relay(
        State(app.clone()),
        Json(
            UaRelayRequest::new(
                ua_account_id.clone(),
                ExecuteArgsMessage {
                    key: passkey.clone(),
                    mws: Box::new(message),
                },
            )
            .unwrap(),
        ),
    )
    .await;

    eprintln!("UA Relay response: {response:?}");

    let response = match response {
        SimpleResponse::Success(response) => response,
        e => {
            panic!("Should succeed: {e:?}");
        }
    };

    eprintln!("Transaction hash: {}", response.transaction_hash);

    poll_tx_status(response.transaction_hash, ua_account_id.clone()).await;

    // Test intents.near contract intraction
    // The actual transaction should fail, because `intents.near` does not
    // exist on the sandbox blockchain, but the relayer should still send the
    // transaction.

    let parameters = load_parameters(ua_account_id.clone(), KeyId::Passkey(passkey.clone())).await;

    let message = create_execute_message(
        &secret_key,
        parameters.next_nonce(),
        "intents.near".parse().unwrap(),
        vec![transaction::FunctionCallAction {
            function_name: "add_public_key".to_string(),
            arguments: b"{}".to_vec().into(),
            amount: NearToken::ZERO,
            gas: near_sdk::Gas::from_tgas(20),
        }
        .into()],
    );

    let response = templar_relayer::route::universal_account::relay::relay(
        State(app.clone()),
        Json(
            UaRelayRequest::new(
                ua_account_id.clone(),
                ExecuteArgsMessage {
                    key: passkey.clone(),
                    mws: Box::new(message),
                },
            )
            .unwrap(),
        ),
    )
    .await;

    let SimpleResponse::Success(result) = response else {
        panic!("Should have succeeded: {response:?}");
    };

    eprintln!("Transaction hash: {}", result.transaction_hash);

    poll_tx_status(result.transaction_hash, ua_account_id.clone()).await;
}

#[rstest]
#[tokio::test]
#[ignore = "Puts tx on testnet. Set ACCOUNT_ID and SECRET_KEY before running."]
pub async fn pyth_updates() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let account_id: AccountId = std::env::var("ACCOUNT_ID").unwrap().parse().unwrap();
    let secret_key: near_crypto::SecretKey = std::env::var("SECRET_KEY").unwrap().parse().unwrap();

    let pyth_args = args::Pyth {
        hermes_url: "https://hermes-beta.pyth.network".to_string(),
        refresh: Duration::from_secs(25),
        oracle_id: "pyth-oracle.testnet".parse().unwrap(),
        update_gas: near_sdk::Gas::from_tgas(300),
        update_deposit: NearToken::from_near(1).saturating_div(100),
    };

    let near = Near::new(
        JsonRpcClient::connect("https://test.rpc.fastnear.com"),
        account_id.clone(),
        vec![near_crypto::InMemorySigner::from_secret_key(
            account_id, secret_key,
        )],
    );

    let cache_args = args::Cache {
        gas_price_refresh: Duration::from_secs(600),
        nonce_refresh: Duration::from_secs(60),
    };

    let kill = watch::Sender::default();

    let cache = Cache::new(near.clone(), cache_args, kill.clone());

    let pyth = Pyth::new(pyth_args.clone(), near.clone(), cache.clone(), kill.clone());

    let price_id = PriceIdentifier(
        hex::decode("f9c0172ba10dfa4d19088d94f5bf61d3b54d5bd7483a322a982e1373ee8ea31b")
            .unwrap()
            .try_into()
            .unwrap(),
    );

    let txid = pyth.update(Box::new([price_id])).await.unwrap();

    eprintln!("Transaction hash: {txid:?}");

    kill.send(()).unwrap();
}

#[rstest]
#[tokio::test]
pub async fn universal_account_reflexive(#[future(awt)] init_test: InitTest) {
    let InitTest {
        app,
        ua_deployer,
        borrow_user,
        ..
    } = init_test;

    // Relay a signed delegate action.

    let fetch_nonce = app
        .relay_near
        .fetch_nonce(
            borrow_user.id().clone(),
            borrow_user.public_key_string().parse().unwrap(),
        )
        .await
        .unwrap();

    // Deploy a universal account.

    let secret_key = p256::SecretKey::random(&mut OsRng);
    let passkey = Passkey(PublicKey(secret_key.public_key()));

    let message = create_message(
        &secret_key,
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .zero()
            .verifying_contract(ua_deployer.account().id().clone())
            .build_salt(),
        Pow::mine(
            CreateUniversalAccount {
                key: passkey.clone().into(),
                block_hash: fetch_nonce.block_hash,
            },
            POW_DIFFICULTY,
            10_000,
        )
        .unwrap(),
    );

    let response = templar_relayer::route::universal_account::create::create(
        State(app.clone()),
        Json(CreateRequest::ExecuteArgs(
            ExecuteArgsMessage {
                key: passkey.clone(),
                mws: Box::new(message),
            }
            .into(),
        )),
    )
    .await;

    eprintln!("UA deploy response: {response:?}");

    let SimpleResponse::Success(response) = response else {
        panic!("Universal account deployment should succeed");
    };

    let ua_account_id = response.account_id.clone();

    poll_tx_status(response.transaction_hash, ua_deployer.account.id.clone()).await;

    // Send an action to the universal account contract

    let load_parameters = async |account_id: AccountId, key: KeyId| {
        app.ua_near
            .load_ua_key(account_id, key)
            .await
            .unwrap()
            .unwrap()
    };

    let parameters = load_parameters(ua_account_id.clone(), KeyId::Passkey(passkey.clone())).await;
    let secret_key_2 = p256::SecretKey::random(&mut OsRng);
    let passkey_2 = Passkey(PublicKey(secret_key_2.public_key()));

    let message = create_execute_message(
        &secret_key,
        parameters.next_nonce(),
        ua_account_id.clone(),
        vec![transaction::FunctionCallAction {
            function_name: "add_key".to_string(),
            arguments: serde_json::to_vec(&json!({
                "key": KeyId::Passkey(passkey_2.clone()),
            }))
            .unwrap()
            .into(),
            amount: NearToken::ZERO,
            gas: near_sdk::Gas::from_tgas(25),
        }
        .into()],
    );

    let response = templar_relayer::route::universal_account::relay::relay(
        State(app.clone()),
        Json(
            UaRelayRequest::new(
                ua_account_id.clone(),
                ExecuteArgsMessage {
                    key: passkey.clone(),
                    mws: Box::new(message),
                },
            )
            .unwrap(),
        ),
    )
    .await;

    eprintln!("UA Relay response: {response:?}");

    let response = match response {
        SimpleResponse::Success(response) => response,
        e => {
            panic!("Should succeed: {e:?}");
        }
    };

    eprintln!("Transaction hash: {}", response.transaction_hash);

    poll_tx_status(response.transaction_hash, ua_account_id.clone()).await;

    // Test intents.near contract intraction
    // The actual transaction should fail, because `intents.near` does not
    // exist on the sandbox blockchain, but the relayer should still send the
    // transaction.

    let parameters =
        load_parameters(ua_account_id.clone(), KeyId::Passkey(passkey_2.clone())).await;

    let message = create_execute_message(
        &secret_key_2,
        parameters.next_nonce(),
        ua_account_id.clone(),
        vec![transaction::FunctionCallAction {
            function_name: "execute".to_string(),
            arguments: b"{}".to_vec().into(),
            amount: NearToken::ZERO,
            gas: near_sdk::Gas::from_tgas(200),
        }
        .into()],
    );

    let response = templar_relayer::route::universal_account::relay::relay(
        State(app.clone()),
        Json(
            UaRelayRequest::new(
                ua_account_id.clone(),
                ExecuteArgsMessage {
                    key: passkey_2.clone(),
                    mws: Box::new(message),
                },
            )
            .unwrap(),
        ),
    )
    .await;

    let SimpleResponse::Rejected { reason } = response else {
        panic!("Should have been rejected: {response:?}");
    };

    assert_eq!(reason, "Recursive `execute` call");
}
