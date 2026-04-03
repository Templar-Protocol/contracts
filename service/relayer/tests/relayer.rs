#![allow(clippy::unwrap_used)]

use std::time::{SystemTime, UNIX_EPOCH};
use std::{collections::HashSet, str::FromStr, time::Duration};

use axum::extract::Query;
use axum::{extract::State, Json};
use clap::Parser;
use near_jsonrpc_client::{methods::tx::TransactionInfo, JsonRpcClient};
use near_primitives::{
    action::{
        delegate::{DelegateAction, SignedDelegateAction},
        Action, FunctionCallAction,
    },
    views::TxExecutionStatus,
};
use near_sdk::{
    env::sha256_array,
    json_types::Base64VecU8,
    serde_json::{self, json},
    AccountId, NearToken,
};
use near_workspaces::{network::Sandbox, Account, Worker};
use p256::{
    ecdsa::{signature::Signer, SigningKey},
    elliptic_curve::rand_core::OsRng,
};
use rstest::{fixture, rstest};
use tokio::sync::watch;

use templar_common::{
    oracle::{
        price_transformer::{self, ProxyPriceTransformer},
        proxy::{Proxy, Source},
        pyth::{self, PriceIdentifier, PythTimestamp},
        redstone, OracleRequest,
    },
    registry::DeployMode,
};
use templar_relayer::{
    app::{args, App, Configuration},
    cache::Cache,
    client::{near::Near, oracle},
    route::{
        get_market_prices::GetMarketPricesRequest,
        relay::RelayRequest as SdaRelayRequest,
        universal_account::{
            create::{CreateRequest, CreateUniversalAccount},
            pow::Pow,
            relay::RelayRequest as UaRelayRequest,
        },
        update_prices::UpdatePricesRequest,
        update_prices::UpdatePricesResponse,
        SimpleResponse,
    },
    ViewMarketPrices,
};
use templar_universal_account::{
    authentication::{
        passkey::{
            self,
            data::{AuthenticatorData, ClientDataJson},
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
    worker: Worker<Sandbox>,
    app: App,
    borrow_asset: FtController,
    collateral_asset: FtController,
    ua_registry: RegistryController,
    market_registry: RegistryController,
    borrow_user: Account,
    relay_user: Account,
}

impl InitTest {
    async fn market_with_pyth_oracle(&mut self) -> (MarketController, MockOracleController) {
        accounts!(self.worker, protocol_yield_user, pyth_oracle);

        let config = market_configuration(
            pyth_oracle.id().clone(),
            self.borrow_asset.id().clone(),
            self.collateral_asset.id().clone(),
            protocol_yield_user.id().clone(),
            templar_common::market::YieldWeights::new_with_supply_weight(8),
        );

        let pyth_oracle = MockOracleController::deploy(pyth_oracle);
        let market = async {
            let m = self
                .market_registry
                .deploy(
                    self.market_registry.account(),
                    "market_w_pyth",
                    "market",
                    serde_json::to_vec(&json!({"configuration": config})).unwrap(),
                    vec![],
                )
                .await;
            MarketController::attach(&self.worker, m)
        };
        let (pyth_oracle, market) = tokio::join!(pyth_oracle, market);

        self.app.load_markets().await;

        (market, pyth_oracle)
    }

    async fn setup_proxy_oracle_with_redstone(
        &self,
        proxy_oracle: &ProxyOracleController,
    ) -> RedStoneAdapterController {
        accounts!(self.worker, redstone_adapter);
        let redstone_adapter =
            RedStoneAdapterController::deploy(redstone_adapter, redstone::config::prod()).await;

        set_proxy(
            proxy_oracle,
            DEFAULT_COLLATERAL_PRICE_ID,
            OracleRequest::redstone(redstone_adapter.id().clone(), "BTC"),
        )
        .await;
        set_proxy(
            proxy_oracle,
            DEFAULT_BORROW_PRICE_ID,
            OracleRequest::redstone(redstone_adapter.id().clone(), "USDC"),
        )
        .await;

        redstone_adapter
    }

    async fn setup_proxy_oracle_with_pyth(
        &self,
        proxy_oracle: &ProxyOracleController,
    ) -> MockOracleController {
        accounts!(self.worker, pyth_oracle);
        let pyth_oracle = MockOracleController::deploy(pyth_oracle).await;

        set_pyth_price(&pyth_oracle, DEFAULT_COLLATERAL_PRICE_ID, fresh_price(1)).await;
        set_pyth_price(&pyth_oracle, DEFAULT_BORROW_PRICE_ID, fresh_price(1)).await;

        set_proxy(
            proxy_oracle,
            DEFAULT_COLLATERAL_PRICE_ID,
            OracleRequest::pyth(
                pyth_oracle.id().clone(),
                test_utils::DEFAULT_COLLATERAL_PRICE_ID,
            ),
        )
        .await;
        set_proxy(
            proxy_oracle,
            DEFAULT_BORROW_PRICE_ID,
            ProxyPriceTransformer::lst(
                OracleRequest::pyth(
                    pyth_oracle.id().clone(),
                    test_utils::DEFAULT_BORROW_PRICE_ID,
                ),
                24,
                price_transformer::Call::new_simple(self.borrow_asset.id(), "redemption_rate"),
            ),
        )
        .await;

        pyth_oracle
    }

    async fn market_proxy(&mut self) -> (MarketController, ProxyOracleController) {
        accounts!(self.worker, protocol_yield_user, proxy_oracle);

        let config = market_configuration(
            proxy_oracle.id().clone(),
            self.borrow_asset.id().clone(),
            self.collateral_asset.id().clone(),
            protocol_yield_user.id().clone(),
            templar_common::market::YieldWeights::new_with_supply_weight(8),
        );

        let proxy_oracle = ProxyOracleController::deploy(proxy_oracle);
        let market = async {
            let m = self
                .market_registry
                .deploy(
                    self.market_registry.account(),
                    "market_w_proxy",
                    "market",
                    serde_json::to_vec(&json!({"configuration": config})).unwrap(),
                    vec![],
                )
                .await;
            MarketController::attach(&self.worker, m)
        };
        let (proxy_oracle, market) = tokio::join!(proxy_oracle, market);

        self.app.load_markets().await;

        (market, proxy_oracle)
    }

    pub async fn market_proxy_pyth(
        &mut self,
    ) -> (
        MarketController,
        ProxyOracleController,
        MockOracleController,
    ) {
        let (market, proxy_oracle) = self.market_proxy().await;
        let pyth_oracle = self.setup_proxy_oracle_with_pyth(&proxy_oracle).await;
        self.app.load_markets().await;
        (market, proxy_oracle, pyth_oracle)
    }

    pub async fn market_proxy_redstone(
        &mut self,
    ) -> (
        MarketController,
        ProxyOracleController,
        RedStoneAdapterController,
    ) {
        let (market, proxy_oracle) = self.market_proxy().await;
        let redstone_adapter = self.setup_proxy_oracle_with_redstone(&proxy_oracle).await;
        self.app.load_markets().await;
        (market, proxy_oracle, redstone_adapter)
    }
}

async fn spawn_router(app: App) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, templar_relayer::router(app))
            .await
            .unwrap();
    });
    (format!("http://{address}"), server)
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

fn fresh_price(price: i64) -> pyth::Price {
    #[allow(clippy::cast_possible_wrap)]
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    pyth::Price {
        price: price.into(),
        conf: 0_u64.into(),
        expo: -4,
        publish_time: PythTimestamp::from_secs(now),
    }
}

async fn init_relayer_app(
    worker: &Worker<Sandbox>,
    registry_id: &AccountId,
    relay_user: &Account,
    ua_account: &Account,
) -> App {
    let app = App::new(
        Configuration::parse_from([
            "relayer",
            "--rpc-url",
            &worker.rpc_addr(),
            "--database-url",
            "postgres://relayeruser:password@0.0.0.0:5432/relayer",
            "--monitor-registry-id",
            registry_id.as_ref(),
            "--relay-account-id",
            relay_user.id().as_ref(),
            "--relay-secret-key",
            &relay_user.secret_key().to_string(),
            "--ua-account-id",
            ua_account.id().as_ref(),
            "--ua-secret-key",
            &ua_account.secret_key().to_string(),
            "--ua-registry-id",
            ua_account.id().as_ref(),
            "--ua-version-key",
            "latest",
            "--ua-chain-id",
            &NEAR_TESTNET_CHAIN_ID.to_string(),
            "--ua-pow-difficulty",
            &POW_DIFFICULTY.to_string(),
            "--intents-id",
            "intents.near",
        ]),
        watch::Sender::default(),
    )
    .unwrap();
    app.database.migrate().await.unwrap();
    app
}

async fn set_pyth_price(
    price_oracle: &MockOracleController,
    price_id: PriceIdentifier,
    price: pyth::Price,
) {
    price_oracle
        .set_pyth_price(price_oracle.contract.as_account(), price_id, Some(price))
        .await;
}

async fn set_proxy(
    proxy_oracle: &ProxyOracleController,
    price_id: PriceIdentifier,
    source: impl Into<Source>,
) {
    proxy_oracle
        .set_proxy(
            proxy_oracle.account(),
            price_id,
            Some(Proxy::median_low([source.into()])),
        )
        .await;
}

#[fixture]
async fn init_test(#[future(awt)] worker: Worker<Sandbox>) -> InitTest {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "templar_relayer=debug,warn",
        ))
        .try_init();

    accounts!(
        worker,
        borrow_asset,
        collateral_asset,
        borrow_user,
        relay_user,
        ua_registry,
        market_registry
    );

    let market_registry = async {
        let r = RegistryController::new(market_registry).await;
        r.add_version(
            r.account(),
            NearToken::from_yoctonear(1),
            "market",
            DeployMode::Normal,
            MarketController::wasm().await,
        )
        .await;
        r
    };

    let ua_registry = async {
        let r = RegistryController::new(ua_registry).await;
        r.add_version(
            r.contract().as_account(),
            NearToken::from_near(80),
            "latest",
            DeployMode::GlobalHash,
            UniversalAccountController::wasm().await,
        )
        .await;
        r
    };

    let borrow_asset = FtController::deploy(borrow_asset, "Borrow Asset", "BORROW");
    let collateral_asset = FtController::deploy(collateral_asset, "Collateral Asset", "COLLATERAL");
    let (borrow_asset, collateral_asset, market_registry, ua_registry) =
        tokio::join!(borrow_asset, collateral_asset, market_registry, ua_registry);

    borrow_asset
        .set_redemption_rate(borrow_asset.contract.as_account(), 2 * 10u128.pow(24))
        .await;

    let app = init_relayer_app(
        &worker,
        market_registry.contract().id(),
        &relay_user,
        ua_registry.account(),
    )
    .await;

    InitTest {
        worker,
        app,
        borrow_asset,
        collateral_asset,
        ua_registry,
        market_registry,
        borrow_user,
        relay_user,
    }
}

#[rstest]
#[tokio::test]
pub async fn delegate_action(#[future(awt)] mut init_test: InitTest) {
    let (market, _) = init_test.market_with_pyth_oracle().await;
    let InitTest {
        worker,
        app,
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
        receiver_id: market.contract().id().clone(),
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
            update_prices: false,
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
pub async fn update_prices_rejects_empty_request(#[future(awt)] init_test: InitTest) {
    let InitTest { app, .. } = init_test;

    let response = templar_relayer::route::update_prices::update_prices(
        State(app),
        Json(UpdatePricesRequest { market_ids: vec![] }),
    )
    .await;

    let SimpleResponse::Rejected { reason } = response else {
        panic!("Empty request should be rejected");
    };

    assert_eq!(reason, "market_ids must not be empty");
}

#[rstest]
#[tokio::test]
pub async fn update_prices_rejects_unknown_market(#[future(awt)] init_test: InitTest) {
    let InitTest { app, .. } = init_test;

    let response = templar_relayer::route::update_prices::update_prices(
        State(app),
        Json(UpdatePricesRequest {
            market_ids: vec!["unknown-market.test.near".parse().unwrap()],
        }),
    )
    .await;

    let SimpleResponse::Rejected { reason } = response else {
        panic!("Unknown market should be rejected");
    };

    assert_eq!(reason, "Unknown market: unknown-market.test.near");
}

#[rstest]
#[tokio::test]
pub async fn requires_network_router_serves_price_routes(#[future(awt)] mut init_test: InitTest) {
    let (market, _proxy_oracle, _redstone_adapter) = init_test.market_proxy_redstone().await;
    let InitTest { app, .. } = init_test;

    let (base_url, server) = spawn_router(app).await;
    let client = reqwest::Client::new();

    let update_response = client
        .post(format!("{base_url}/update_prices"))
        .json(&UpdatePricesRequest {
            market_ids: vec![market.id().clone(), market.id().clone()],
        })
        .send()
        .await
        .unwrap();
    assert!(update_response.status().is_success());

    let SimpleResponse::Success(update_response) = update_response
        .json::<SimpleResponse<UpdatePricesResponse>>()
        .await
        .unwrap()
    else {
        panic!("update_prices should succeed");
    };
    assert_eq!(update_response.market_ids, vec![market.id().clone()]);

    let prices_response = client
        .get(format!("{base_url}/market_prices"))
        .query(&GetMarketPricesRequest {
            market_id: market.id().clone(),
        })
        .send()
        .await
        .unwrap();
    assert!(prices_response.status().is_success());

    let SimpleResponse::Success(prices) = prices_response
        .json::<SimpleResponse<ViewMarketPrices>>()
        .await
        .unwrap()
    else {
        panic!("market_prices should succeed");
    };
    assert!(prices.borrow.is_some());
    assert!(prices.collateral.is_some());

    server.abort();
}

#[rstest]
#[tokio::test]
pub async fn market_prices_returns_direct_market_prices(#[future(awt)] mut init_test: InitTest) {
    let (market, pyth_oracle) = init_test.market_with_pyth_oracle().await;
    let InitTest { app, .. } = init_test;

    let borrow_price = fresh_price(345_600);
    let collateral_price = fresh_price(1_234_500);

    set_pyth_price(
        &pyth_oracle,
        test_utils::DEFAULT_BORROW_PRICE_ID,
        borrow_price.clone(),
    )
    .await;
    set_pyth_price(
        &pyth_oracle,
        test_utils::DEFAULT_COLLATERAL_PRICE_ID,
        collateral_price.clone(),
    )
    .await;

    let response = templar_relayer::route::get_market_prices::get_market_prices(
        State(app),
        Query(GetMarketPricesRequest {
            market_id: market.contract().id().clone(),
        }),
    )
    .await;

    let SimpleResponse::Success(response) = response else {
        panic!("market_prices should succeed");
    };

    assert_eq!(response.borrow, Some(borrow_price));
    assert_eq!(response.collateral, Some(collateral_price));
}

#[rstest]
#[tokio::test]
pub async fn market_prices_returns_none_for_missing_asset_price(
    #[future(awt)] mut init_test: InitTest,
) {
    let (market, pyth_oracle) = init_test.market_with_pyth_oracle().await;
    let InitTest { app, .. } = init_test;

    let collateral_price = fresh_price(1_234_500);
    set_pyth_price(
        &pyth_oracle,
        test_utils::DEFAULT_COLLATERAL_PRICE_ID,
        collateral_price.clone(),
    )
    .await;

    let response = templar_relayer::route::get_market_prices::get_market_prices(
        State(app),
        Query(GetMarketPricesRequest {
            market_id: market.contract().id().clone(),
        }),
    )
    .await;

    let SimpleResponse::Success(response) = response else {
        panic!("market_prices should succeed");
    };

    assert_eq!(response.borrow, None);
    assert_eq!(response.collateral, Some(collateral_price));
}

#[rstest]
#[tokio::test]
pub async fn market_prices_returns_proxy_intermediate_prices(
    #[future(awt)] mut init_test: InitTest,
) {
    let (market, _proxy_oracle, pyth_oracle) = init_test.market_proxy_pyth().await;
    let InitTest { app, .. } = init_test;

    set_pyth_price(
        &pyth_oracle,
        DEFAULT_COLLATERAL_PRICE_ID,
        fresh_price(2_500_000),
    )
    .await;
    set_pyth_price(
        &pyth_oracle,
        DEFAULT_BORROW_PRICE_ID,
        fresh_price(1_000_000),
    )
    .await;

    let response = templar_relayer::route::get_market_prices::get_market_prices(
        State(app),
        Query(GetMarketPricesRequest {
            market_id: market.id().clone(),
        }),
    )
    .await;

    let response = match response {
        SimpleResponse::Success(response) => response,
        e => {
            panic!("market_prices should succeed: {e:#?}");
        }
    };

    assert_eq!(response.collateral.as_ref().unwrap().price.0, 2_500_000);
    assert_eq!(
        response.borrow.as_ref().unwrap().price.0,
        1_000_000 * 2 /* redemption rate */
    );
}

#[rstest]
#[tokio::test]
pub async fn requires_network_update_prices_updates_redstone_market(
    #[future(awt)] mut init_test: InitTest,
) {
    let (market, _proxy_oracle, redstone_adapter) = init_test.market_proxy_redstone().await;
    let InitTest { app, .. } = init_test;

    let usdc = redstone::FeedId::from("USDC");
    let btc = redstone::FeedId::from("BTC");

    let price_data_before = redstone_adapter
        .read_price_data(vec![usdc.clone(), btc.clone()])
        .await;
    assert!(price_data_before.is_empty());

    let response = templar_relayer::route::update_prices::update_prices(
        State(app.clone()),
        Json(UpdatePricesRequest {
            market_ids: vec![market.id().clone(), market.id().clone()],
        }),
    )
    .await;

    let SimpleResponse::Success(response) = response else {
        panic!("update_prices should succeed");
    };
    assert_eq!(response.market_ids, vec![market.id().clone()]);

    let accounts = app.accounts.read().await;
    let market_data = accounts.market_data.get(market.id()).unwrap();
    assert!(market_data
        .borrow
        .update_oracle
        .contains(&OracleRequest::redstone(
            redstone_adapter.id().clone(),
            usdc.clone(),
        )));
    assert!(market_data
        .collateral
        .update_oracle
        .contains(&OracleRequest::redstone(
            redstone_adapter.id().clone(),
            btc.clone(),
        )));
    drop(accounts);

    let SimpleResponse::Success(prices) =
        templar_relayer::route::get_market_prices::get_market_prices(
            State(app),
            Query(GetMarketPricesRequest {
                market_id: market.id().clone(),
            }),
        )
        .await
    else {
        panic!("get_market_prices should succeed");
    };
    let Some(borrow) = prices.borrow else {
        panic!("borrow price should resolve to USDC");
    };
    let Some(collateral) = prices.collateral else {
        panic!("collateral price should resolve to BTC");
    };

    let price_data_after = redstone_adapter
        .read_price_data(vec![usdc.clone(), btc.clone()])
        .await;
    assert!(price_data_after.contains_key(&usdc));
    assert!(price_data_after.contains_key(&btc));
    assert_eq!(borrow, price_data_after[&usdc].to_pyth_price().unwrap());
    assert_eq!(collateral, price_data_after[&btc].to_pyth_price().unwrap());
}

#[rstest]
#[tokio::test]
pub async fn universal_account_regression_0_2_0(#[future(awt)] mut init_test: InitTest) {
    let (market, _) = init_test.market_with_pyth_oracle().await;
    let InitTest { worker, app, .. } = init_test;

    let secret_key = p256::SecretKey::from_bytes(&[0xa8; 32].into()).unwrap();
    let passkey = passkey::VerifyKey(PublicKey(secret_key.public_key()));

    let ua = worker
        .dev_deploy(UniversalAccountController::wasm_0_2_0())
        .await
        .unwrap();

    ua.call("new")
        .args_json(json!({ "key": KeyId::Passkey(passkey.clone()) }))
        .transact()
        .await
        .unwrap()
        .unwrap();

    let parameters = app
        .ua_near
        .load_ua_key(ua.id().clone(), KeyId::Passkey(passkey.clone()))
        .await
        .unwrap()
        .unwrap();

    app.database
        .create_account(ua.id(), NearToken::from_near(1).saturating_div(4))
        .await
        .unwrap();

    let message = serde_json::to_string(&json!({
        "parameters": {
            "block_height": parameters.block_height,
            "index": "0",
            "nonce": "1",
        },
        "account_id": ua.id(),
        "payload": [{
            "receiver_id": market.contract().id(),
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
        &[
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
            account_id: ua.id().clone(),
            args: serde_json::from_str(&args).unwrap(),
            storage_deposit: HashSet::default(),
            update_prices: false,
        }),
    )
    .await;

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
                sender_account_id: ua.id().clone(),
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
pub async fn universal_account(#[future(awt)] mut init_test: InitTest) {
    let (market, _) = init_test.market_with_pyth_oracle().await;
    let InitTest {
        worker,
        app,
        ua_registry,
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
    let passkey = passkey::VerifyKey(PublicKey(secret_key.public_key()));

    let message = create_message(
        &secret_key,
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .zero()
            .verifying_contract(ua_registry.contract().id().clone())
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

    let status = worker
        .tx_status(
            TransactionInfo::TransactionId {
                tx_hash: response.transaction_hash,
                sender_account_id: ua_registry.contract().id().clone(),
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
        market.contract().id().clone(),
        vec![transaction::FunctionCallAction {
            function_name: "apply_interest".to_string(),
            arguments: b"{}".to_vec().into(),
            amount: NearToken::from_near(0),
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
            amount: NearToken::from_near(0),
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

    let status = worker
        .tx_status(
            TransactionInfo::TransactionId {
                tx_hash: result.transaction_hash,
                sender_account_id: ua_account_id.clone(),
            },
            TxExecutionStatus::Final,
        )
        .await;

    eprintln!("Status: {status:?}");
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

    let pyth_args = args::PythConfig {
        hermes_url: "https://hermes-beta.pyth.network".to_string(),
        refresh: Duration::from_secs(25),
        update_gas: near_sdk::Gas::from_tgas(300),
        update_deposit: NearToken::from_near(1).saturating_div(100),
        timeout: Duration::from_secs(10),
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

    let pyth =
        oracle::PythSpec::handle(pyth_args.clone(), near.clone(), cache.clone(), kill.clone());

    let price_id = PriceIdentifier(
        hex::decode("f9c0172ba10dfa4d19088d94f5bf61d3b54d5bd7483a322a982e1373ee8ea31b")
            .unwrap()
            .try_into()
            .unwrap(),
    );

    let txid = pyth
        .update("pyth-oracle.testnet".parse().unwrap(), Box::new([price_id]))
        .await
        .unwrap();

    eprintln!("Transaction hash: {txid:?}");

    kill.send(()).unwrap();
}

#[rstest]
#[tokio::test]
pub async fn universal_account_reflexive(#[future(awt)] init_test: InitTest) {
    let InitTest {
        worker,
        app,
        ua_registry,
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
    let passkey = passkey::VerifyKey(PublicKey(secret_key.public_key()));

    let message = create_message(
        &secret_key,
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .zero()
            .verifying_contract(ua_registry.contract().id().clone())
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

    let status = worker
        .tx_status(
            TransactionInfo::TransactionId {
                tx_hash: response.transaction_hash,
                sender_account_id: ua_registry.contract().id().clone(),
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

    let load_parameters = async |account_id: AccountId, key: KeyId| {
        app.ua_near
            .load_ua_key(account_id, key)
            .await
            .unwrap()
            .unwrap()
    };

    let parameters = load_parameters(ua_account_id.clone(), KeyId::Passkey(passkey.clone())).await;
    let secret_key_2 = p256::SecretKey::random(&mut OsRng);
    let passkey_2 = passkey::VerifyKey(PublicKey(secret_key_2.public_key()));

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
            amount: NearToken::from_near(0),
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
            amount: NearToken::from_near(0),
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
