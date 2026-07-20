#![allow(clippy::unwrap_used)]

use std::{collections::HashMap, collections::HashSet, str::FromStr};

use axum::extract::Query;
use axum::{extract::State, Json};
use clap::Parser;
use near_primitives::{
    action::{
        delegate::{DelegateAction, SignedDelegateAction},
        Action, FunctionCallAction,
    },
    hash::CryptoHash,
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

use templar_common::{
    market::YieldWeights,
    oracle::{
        pyth::{self, OracleResponse, PriceIdentifier, PythTimestamp},
        redstone::{FeedData, FeedId},
    },
    registry::DeployMode,
};
use templar_gateway_testing::{harness, owned_harness, ManagedAccountId, SandboxHarness};
use templar_proxy_oracle_kernel::proxy::{FreshnessFilter, Proxy};
use templar_proxy_oracle_near_common::{
    input::{ProxyPriceTransformer, Source},
    price_transformer,
    request::OracleRequest,
};
use templar_relayer::{
    app::{App, Configuration, SubmitError},
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

use templar_gateway_testing::wasm::UNIVERSAL_ACCOUNT_0_2_0;
use test_utils::{market_configuration, DEFAULT_BORROW_PRICE_ID, DEFAULT_COLLATERAL_PRICE_ID};

mod common;

const POW_DIFFICULTY: usize = 6;

const MARKET_VERSION: &str = "market";

struct AccessKeyInfo {
    nonce: u64,
    block_height: u64,
    block_hash: CryptoHash,
}

/// Fetch an account's access-key nonce and the current block reference through
/// the gateway's typed specs (`account.getAccessKey` + `chain.getBlock`) — the
/// same path the relayer itself uses, rather than a bespoke RPC client.
async fn view_access_key(
    gateway: &templar_gateway_client::Client,
    account_id: &AccountId,
    public_key: near_crypto::PublicKey,
) -> AccessKeyInfo {
    use templar_gateway_methods_spec::{account, chain};

    let public_key: near_api::types::PublicKey = public_key.to_string().parse().unwrap();
    let key = gateway
        .read(account::GetAccessKey {
            account_id: account_id.as_str().parse().unwrap(),
            public_key: public_key.into(),
        })
        .await
        .unwrap();
    let block = gateway
        .read(chain::GetBlock { block_hash: None })
        .await
        .unwrap();
    AccessKeyInfo {
        nonce: key.nonce,
        block_height: block.height,
        block_hash: CryptoHash(block.hash.0 .0),
    }
}

/// Every harness account shares the fixed test key, so the relay/UA signer for
/// the `App` configuration is just that key plus the account id.
struct InitTest {
    harness: SandboxHarness,
    app: App,
    borrow_asset: AccountId,
    collateral_asset: AccountId,
    ua_registry: AccountId,
    market_registry: AccountId,
    borrow_user: AccountId,
    relay_user: AccountId,
}

impl InitTest {
    /// Deploy a market named `name` (through the monitored registry) backed by
    /// `oracle`, then refresh the relayer's market view.
    async fn deploy_market_backed_by(&mut self, name: &str, oracle: &AccountId) -> AccountId {
        let protocol_yield_user = common::create_account(&self.harness, "protocol-yield")
            .await
            .unwrap();
        let config = market_configuration(
            oracle.clone(),
            self.borrow_asset.clone(),
            self.collateral_asset.clone(),
            protocol_yield_user,
            YieldWeights::new_with_supply_weight(8),
        );
        let market = self.deploy_market_via_registry(name, &config).await;
        self.app.load_markets().await;
        market
    }

    /// Deploy a market backed by a fresh mock Pyth oracle.
    async fn market_with_pyth_oracle(&mut self) -> (AccountId, AccountId) {
        let pyth_oracle = self
            .harness
            .deploy_mock_oracle("pyth-oracle")
            .await
            .unwrap();
        let market = self
            .deploy_market_backed_by("market_w_pyth", &pyth_oracle)
            .await;
        (market, pyth_oracle)
    }

    /// Deploy a market backed by a proxy oracle (empty proxies for now).
    async fn market_proxy(&mut self) -> (AccountId, AccountId) {
        let proxy_oracle = self.harness.deploy_proxy_oracle().await.unwrap();
        let market = self
            .deploy_market_backed_by("market_w_proxy", &proxy_oracle)
            .await;
        (market, proxy_oracle)
    }

    async fn setup_proxy_oracle_with_redstone(&self, proxy_oracle: &AccountId) -> AccountId {
        let redstone_adapter = self
            .harness
            .deploy_redstone_adapter("redstone-adapter")
            .await
            .unwrap();

        set_proxy(
            &self.harness,
            proxy_oracle,
            DEFAULT_COLLATERAL_PRICE_ID,
            OracleRequest::redstone(redstone_adapter.clone(), "BTC"),
        )
        .await;
        set_proxy(
            &self.harness,
            proxy_oracle,
            DEFAULT_BORROW_PRICE_ID,
            OracleRequest::redstone(redstone_adapter.clone(), "USDC"),
        )
        .await;

        redstone_adapter
    }

    async fn setup_proxy_oracle_with_pyth(&self, proxy_oracle: &AccountId) -> AccountId {
        let pyth_oracle = self
            .harness
            .deploy_mock_oracle("pyth-oracle")
            .await
            .unwrap();

        set_pyth_price(
            &self.harness,
            &pyth_oracle,
            DEFAULT_COLLATERAL_PRICE_ID,
            fresh_price(&self.harness, 1).await,
        )
        .await;
        set_pyth_price(
            &self.harness,
            &pyth_oracle,
            DEFAULT_BORROW_PRICE_ID,
            fresh_price(&self.harness, 1).await,
        )
        .await;

        set_proxy(
            &self.harness,
            proxy_oracle,
            DEFAULT_COLLATERAL_PRICE_ID,
            OracleRequest::pyth(pyth_oracle.clone(), DEFAULT_COLLATERAL_PRICE_ID),
        )
        .await;
        set_proxy(
            &self.harness,
            proxy_oracle,
            DEFAULT_BORROW_PRICE_ID,
            ProxyPriceTransformer::lst(
                OracleRequest::pyth(pyth_oracle.clone(), DEFAULT_BORROW_PRICE_ID),
                24,
                price_transformer::Call::new_simple(&self.borrow_asset, "redemption_rate"),
            ),
        )
        .await;

        pyth_oracle
    }

    async fn market_proxy_pyth(&mut self) -> (AccountId, AccountId, AccountId) {
        let (market, proxy_oracle) = self.market_proxy().await;
        let pyth_oracle = self.setup_proxy_oracle_with_pyth(&proxy_oracle).await;
        self.app.load_markets().await;
        (market, proxy_oracle, pyth_oracle)
    }

    async fn market_proxy_redstone(&mut self) -> (AccountId, AccountId, AccountId) {
        let (market, proxy_oracle) = self.market_proxy().await;
        let redstone_adapter = self.setup_proxy_oracle_with_redstone(&proxy_oracle).await;
        self.app.load_markets().await;
        (market, proxy_oracle, redstone_adapter)
    }

    /// Deploy a market from the monitored registry's `market` version at
    /// `{name}.{market_registry}`, returning its account id.
    async fn deploy_market_via_registry(
        &self,
        name: &str,
        config: &templar_common::market::MarketConfiguration,
    ) -> AccountId {
        let deployer = self.harness.registry_signer_account_id.clone();
        self.harness
            .registry_deploy(
                &deployer,
                &self.market_registry,
                name,
                MARKET_VERSION,
                serde_json::to_vec(&json!({ "configuration": config })).unwrap(),
                None,
                NearToken::from_near(10),
            )
            .await
            .unwrap();
        format!("{name}.{}", self.market_registry).parse().unwrap()
    }
}

async fn set_pyth_price(
    harness: &SandboxHarness,
    oracle: &AccountId,
    price_id: PriceIdentifier,
    price: pyth::Price,
) {
    harness
        .set_mock_oracle_pyth_price(oracle.clone(), price_id, Some(price))
        .await
        .unwrap();
}

async fn set_proxy(
    harness: &SandboxHarness,
    proxy_oracle: &AccountId,
    price_id: PriceIdentifier,
    source: impl Into<Source>,
) {
    harness
        .admin_set_proxy(
            proxy_oracle.clone(),
            price_id,
            Some(Proxy::median_low([source.into()], FreshnessFilter::empty())),
        )
        .await
        .unwrap();
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

/// A Pyth price published one second ago on the *chain's* clock, not the host's
/// — see [`SandboxHarness::chain_timestamp`].
#[allow(clippy::cast_possible_wrap)]
async fn fresh_price(harness: &SandboxHarness, price: i64) -> pyth::Price {
    let now = harness
        .chain_timestamp()
        .await
        .unwrap()
        .as_secs()
        .saturating_sub(1) as i64;

    pyth::Price {
        price: price.into(),
        conf: 0_u64.into(),
        expo: -4,
        publish_time: PythTimestamp::from_secs(now),
    }
}

async fn init_relayer_app(
    harness: &SandboxHarness,
    registry_id: &AccountId,
    relay_user: &AccountId,
    ua_account: &AccountId,
) -> App {
    let rpc_url = harness.network.rpc_endpoints[0].url.to_string();
    let chain_id = NEAR_TESTNET_CHAIN_ID.to_string();
    let pow_difficulty = POW_DIFFICULTY.to_string();

    let app = App::new(
        Configuration::parse_from([
            "relayer",
            "--rpc-url",
            &rpc_url,
            "--database-url",
            "postgres://relayeruser:password@0.0.0.0:5432/relayer",
            "--monitor-registry-id",
            registry_id.as_ref(),
            "--relay-account-id",
            relay_user.as_ref(),
            "--relay-secret-key",
            common::TEST_SECRET_KEY,
            "--ua-account-id",
            ua_account.as_ref(),
            "--ua-secret-key",
            common::TEST_SECRET_KEY,
            "--ua-registry-id",
            ua_account.as_ref(),
            "--ua-version-key",
            "latest",
            "--ua-chain-id",
            &chain_id,
            "--ua-pow-difficulty",
            &pow_difficulty,
            "--intents-id",
            "intents.near",
            // The relayer hosts the gateway's Lazer source, so a Lazer API key is required.
            "--pyth-lazer-api-key",
            "test-token",
        ]),
        watch::Sender::default(),
    )
    .await
    .unwrap();
    app.database.migrate().await.unwrap();
    app
}

#[fixture]
async fn init_test(#[future(awt)] harness: SandboxHarness) -> InitTest {
    init_with(harness).await
}

/// A *dedicated* node, for the tests that create a universal account: that route
/// ages a block reference against the *host* clock, which a pooled node's
/// `fast_forward`-skewed chain clock makes it read as "too old". Only those two
/// tests pay for the node boot — see [`SandboxHarness::start_owned`].
#[fixture]
async fn init_test_owned(#[future(awt)] owned_harness: SandboxHarness) -> InitTest {
    init_with(owned_harness).await
}

async fn init_with(harness: SandboxHarness) -> InitTest {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "templar_relayer=debug,warn",
        ))
        .try_init();

    let borrow_asset = harness.deploy_ft("borrow-asset", "Borrow Asset", "BORROW");
    let collateral_asset = harness.deploy_ft("collateral-asset", "Collateral Asset", "COLLATERAL");
    let borrow_user = common::create_account(&harness, "borrow-user");
    let relay_user = common::create_account(&harness, "relay-user");
    // The UA registry account doubles as the relayer's UA deployer account.
    let ua_registry = common::deploy_registry(&harness, "ua-registry");
    let market_registry = harness.deploy_registry();

    let (borrow_asset, collateral_asset, borrow_user, relay_user, ua_registry, market_registry) = tokio::join!(
        borrow_asset,
        collateral_asset,
        borrow_user,
        relay_user,
        ua_registry,
        market_registry
    );
    let borrow_asset = borrow_asset.unwrap();
    let collateral_asset = collateral_asset.unwrap();
    let borrow_user = borrow_user.unwrap();
    let relay_user = relay_user.unwrap();
    let ua_registry = ua_registry.unwrap();
    let market_registry = market_registry.unwrap();

    // Register the market and universal-account code versions on their
    // registries. `add_version` asserts a 1-yoctoNEAR deposit for `Normal`
    // (state-stored) code; a `GlobalHash` version pays the global-contract
    // deployment cost from the deposit. The two registries have distinct owner
    // accounts, so their registrations run concurrently.
    let market_deployer = harness.registry_signer_account_id.clone();
    let ua_deployer = ManagedAccountId(ua_registry.clone());
    let market_wasm = templar_gateway_testing::wasm::market().await.to_vec();
    let ua_wasm = templar_gateway_testing::wasm::universal_account()
        .await
        .to_vec();
    let (market_version, ua_version) = tokio::join!(
        harness.registry_add_version(
            &market_deployer,
            &market_registry,
            MARKET_VERSION,
            DeployMode::Normal,
            market_wasm,
            NearToken::from_yoctonear(1),
        ),
        harness.registry_add_version(
            &ua_deployer,
            &ua_registry,
            "latest",
            DeployMode::GlobalHash,
            ua_wasm,
            NearToken::from_near(80),
        ),
    );
    market_version.unwrap();
    ua_version.unwrap();

    // The borrow asset is an LST whose redemption rate the proxy oracle reads.
    common::call(
        &harness.network,
        &borrow_asset,
        &borrow_asset,
        "set_redemption_rate",
        json!({ "redemption_rate": near_sdk::json_types::U128(2 * 10u128.pow(24)) }),
        20,
        NearToken::from_yoctonear(0),
    )
    .await
    .unwrap();

    let app = init_relayer_app(&harness, &market_registry, &relay_user, &ua_registry).await;

    InitTest {
        harness,
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
        harness,
        app,
        borrow_user,
        relay_user,
        ..
    } = init_test;

    // Relay a signed delegate action.
    let secret_key = near_crypto::SecretKey::from_str(common::TEST_SECRET_KEY).unwrap();
    let public_key = secret_key.public_key();

    let fetch_nonce = view_access_key(&app.gateway, &borrow_user, public_key.clone()).await;

    let delegate_action = DelegateAction {
        sender_id: borrow_user.clone(),
        receiver_id: market.clone(),
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
        public_key: public_key.clone(),
    };

    let signature = secret_key.sign(&delegate_action.get_nep461_hash().0);

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
        }),
    )
    .await;

    let SimpleResponse::Success(response) = response else {
        panic!("Relay attempt should succeed");
    };

    common::assert_tx_succeeded(&harness.network, response.transaction_hash, &relay_user)
        .await
        .unwrap();
}

// Empty-request and unknown-market rejection for `/update_prices` and
// `/get_market_prices` are covered off-node by the pure `validate_market_ids`
// tests in `src/route/mod.rs`.

#[rstest]
#[tokio::test]
pub async fn market_prices_fails_when_known_market_configuration_cannot_be_read(
    #[future(awt)] init_test: InitTest,
) {
    let InitTest { app, .. } = init_test;
    let market_id: AccountId = "missing-known-market.test.near".parse().unwrap();
    app.accounts
        .write()
        .await
        .market_ids
        .insert(market_id.clone());

    let response = templar_relayer::route::get_market_prices::get_market_prices(
        State(app),
        Query(GetMarketPricesRequest { market_id }),
    )
    .await;

    let SimpleResponse::Failure { error } = response else {
        panic!("Known market configuration read failure should be a route failure");
    };

    assert_eq!(error, "Failed to load market configuration");
}

async fn assert_storage_deposit_planning_failure(app: &App, payer: &AccountId) {
    use templar_gateway_methods_spec::storage;

    let result = app
        .execute_and_account(
            payer.clone(),
            app.args.relay.account_id.clone(),
            NearToken::from_millinear(1),
            NearToken::from_near(0),
            storage::EnsureDeposit {
                contract_id: "does-not-exist.test.near".parse().unwrap(),
                account_id: payer.clone(),
                mode: storage::EnsureDepositMode::Registered,
            },
        )
        .await;

    assert!(
        matches!(result, Err(SubmitError::Gateway(_))),
        "planning should fail: {result:?}",
    );
}

/// A planning failure releases the reservation immediately: a first-seen account
/// keeps the full starting allowance, an existing account its prior balance.
#[rstest]
#[tokio::test]
async fn planning_failure_creates_missing_account_without_resetting_existing_account(
    #[future(awt)] init_test: InitTest,
) {
    let InitTest { app, .. } = init_test;

    let missing: AccountId = "missing.test.near".parse().unwrap();
    assert_storage_deposit_planning_failure(&app, &missing).await;
    assert_eq!(
        app.database
            .get_available_allowance(&missing)
            .await
            .unwrap()
            .unwrap()
            .as_yoctonear(),
        app.args.relay.starting_allowance_yocto.as_yoctonear(),
    );

    let existing: AccountId = "existing.test.near".parse().unwrap();
    app.database
        .create_account(&existing, NearToken::from_near(100))
        .await
        .unwrap();
    assert_storage_deposit_planning_failure(&app, &existing).await;
    assert_eq!(
        app.database
            .get_available_allowance(&existing)
            .await
            .unwrap()
            .unwrap()
            .as_yoctonear(),
        NearToken::from_near(100).as_yoctonear(),
    );
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
            market_ids: vec![market.clone(), market.clone()],
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
    assert_eq!(update_response.market_ids, vec![market.clone()]);

    let prices_response = client
        .get(format!("{base_url}/market_prices"))
        .query(&GetMarketPricesRequest {
            market_id: market.clone(),
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
    let InitTest { harness, app, .. } = init_test;

    let borrow_price = fresh_price(&harness, 345_600).await;
    let collateral_price = fresh_price(&harness, 1_234_500).await;

    set_pyth_price(
        &harness,
        &pyth_oracle,
        DEFAULT_BORROW_PRICE_ID,
        borrow_price.clone(),
    )
    .await;
    set_pyth_price(
        &harness,
        &pyth_oracle,
        DEFAULT_COLLATERAL_PRICE_ID,
        collateral_price.clone(),
    )
    .await;

    let response = templar_relayer::route::get_market_prices::get_market_prices(
        State(app),
        Query(GetMarketPricesRequest {
            market_id: market.clone(),
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
    let InitTest { harness, app, .. } = init_test;

    let collateral_price = fresh_price(&harness, 1_234_500).await;
    set_pyth_price(
        &harness,
        &pyth_oracle,
        DEFAULT_COLLATERAL_PRICE_ID,
        collateral_price.clone(),
    )
    .await;

    let response = templar_relayer::route::get_market_prices::get_market_prices(
        State(app),
        Query(GetMarketPricesRequest {
            market_id: market.clone(),
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
    let (market, proxy_oracle, pyth_oracle) = init_test.market_proxy_pyth().await;
    let InitTest { harness, app, .. } = init_test;

    set_pyth_price(
        &harness,
        &pyth_oracle,
        DEFAULT_COLLATERAL_PRICE_ID,
        fresh_price(&harness, 2_500_000).await,
    )
    .await;
    set_pyth_price(
        &harness,
        &pyth_oracle,
        DEFAULT_BORROW_PRICE_ID,
        fresh_price(&harness, 1_000_000).await,
    )
    .await;

    harness
        .update_proxy_prices(
            proxy_oracle.clone(),
            vec![DEFAULT_COLLATERAL_PRICE_ID, DEFAULT_BORROW_PRICE_ID],
        )
        .await
        .unwrap();
    let direct_proxy_prices: OracleResponse = common::view(
        &harness.network,
        &proxy_oracle,
        "list_ema_prices_no_older_than",
        json!({
            "price_ids": [DEFAULT_COLLATERAL_PRICE_ID, DEFAULT_BORROW_PRICE_ID],
            "age": 60,
        }),
    )
    .await
    .unwrap();
    assert!(direct_proxy_prices
        .get(&DEFAULT_COLLATERAL_PRICE_ID)
        .is_some_and(|p| p.is_some()));
    assert!(direct_proxy_prices
        .get(&DEFAULT_BORROW_PRICE_ID)
        .is_some_and(|p| p.is_some()));
    let response = templar_relayer::route::get_market_prices::get_market_prices(
        State(app),
        Query(GetMarketPricesRequest {
            market_id: market.clone(),
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
    let InitTest { harness, app, .. } = init_test;

    let usdc = FeedId::from("USDC");
    let btc = FeedId::from("BTC");

    let price_data_before: HashMap<FeedId, FeedData> = common::view(
        &harness.network,
        &redstone_adapter,
        "read_price_data",
        json!({ "feed_ids": [usdc.clone(), btc.clone()] }),
    )
    .await
    .unwrap();
    assert!(price_data_before.is_empty());

    let response = templar_relayer::route::update_prices::update_prices(
        State(app.clone()),
        Json(UpdatePricesRequest {
            market_ids: vec![market.clone(), market.clone()],
        }),
    )
    .await;

    let SimpleResponse::Success(response) = response else {
        panic!("update_prices should succeed");
    };
    assert_eq!(response.market_ids, vec![market.clone()]);

    let SimpleResponse::Success(prices) =
        templar_relayer::route::get_market_prices::get_market_prices(
            State(app),
            Query(GetMarketPricesRequest {
                market_id: market.clone(),
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

    let price_data_after: HashMap<FeedId, FeedData> = common::view(
        &harness.network,
        &redstone_adapter,
        "read_price_data",
        json!({ "feed_ids": [usdc.clone(), btc.clone()] }),
    )
    .await
    .unwrap();
    assert!(price_data_after.contains_key(&usdc));
    assert!(price_data_after.contains_key(&btc));
    assert_eq!(borrow, price_data_after[&usdc].to_pyth_price().unwrap());
    assert_eq!(collateral, price_data_after[&btc].to_pyth_price().unwrap());
}

#[rstest]
#[tokio::test]
pub async fn universal_account_regression_0_2_0(#[future(awt)] mut init_test: InitTest) {
    let (market, _) = init_test.market_with_pyth_oracle().await;
    let InitTest { harness, app, .. } = init_test;

    let secret_key = p256::SecretKey::from_bytes(&[0xa8; 32].into()).unwrap();
    let passkey = passkey::VerifyKey(PublicKey(secret_key.public_key()));

    // Deploy the historical `0.2.0` universal-account wasm to a fresh account.
    let ua = common::create_account(&harness, "ua-0-2-0").await.unwrap();
    common::deploy_code(&harness.network, &ua, UNIVERSAL_ACCOUNT_0_2_0.to_vec())
        .await
        .unwrap();
    common::call(
        &harness.network,
        &ua,
        &ua,
        "new",
        json!({ "key": KeyId::Passkey(passkey.clone()) }),
        30,
        NearToken::from_yoctonear(0),
    )
    .await
    .unwrap();

    let parameters = templar_relayer::route::universal_account::relay::load_ua_key(
        &app,
        ua.clone(),
        KeyId::Passkey(passkey.clone()),
    )
    .await
    .unwrap()
    .unwrap();

    app.database
        .create_account(&ua, NearToken::from_near(1).saturating_div(4))
        .await
        .unwrap();

    let message = serde_json::to_string(&json!({
        "parameters": {
            "block_height": parameters.block_height,
            "index": "0",
            "nonce": "1",
        },
        "account_id": ua,
        "payload": [{
            "receiver_id": market,
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
            account_id: ua.clone(),
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

    common::assert_tx_succeeded(&harness.network, response.transaction_hash, &ua)
        .await
        .unwrap();
}

/// Deploy a universal account through the relayer's `create` route (mining the
/// required PoW against `borrow_user`'s access key) and assert the deployment
/// landed. Returns the new account id and the passkey secret it was created
/// with, for follow-up relays.
async fn create_universal_account(
    app: &App,
    network: &near_api::NetworkConfig,
    ua_registry: &AccountId,
    borrow_user: &AccountId,
) -> (AccountId, p256::SecretKey, passkey::VerifyKey) {
    let borrow_secret_key = near_crypto::SecretKey::from_str(common::TEST_SECRET_KEY).unwrap();
    let fetch_nonce =
        view_access_key(&app.gateway, borrow_user, borrow_secret_key.public_key()).await;

    let secret_key = p256::SecretKey::random(&mut OsRng);
    let passkey = passkey::VerifyKey(PublicKey(secret_key.public_key()));

    let message = create_message(
        &secret_key,
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .zero()
            .verifying_contract(ua_registry.clone())
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

    let SimpleResponse::Success(response) = response else {
        panic!("Universal account deployment should succeed, got: {response:?}");
    };
    let ua_account_id = response.account_id.clone();
    common::assert_tx_succeeded(network, response.transaction_hash, ua_registry)
        .await
        .unwrap();

    (ua_account_id, secret_key, passkey)
}

#[rstest]
#[tokio::test]
pub async fn universal_account(#[future(awt)] mut init_test_owned: InitTest) {
    let (market, _) = init_test_owned.market_with_pyth_oracle().await;
    let InitTest {
        harness,
        app,
        ua_registry,
        borrow_user,
        ..
    } = init_test_owned;

    let (ua_account_id, secret_key, passkey) =
        create_universal_account(&app, &harness.network, &ua_registry, &borrow_user).await;

    // Send an action to the universal account contract

    let load_parameters = async |account_id: AccountId, key: KeyId| {
        templar_relayer::route::universal_account::relay::load_ua_key(&app, account_id, key)
            .await
            .unwrap()
            .unwrap()
    };

    let parameters = load_parameters(ua_account_id.clone(), KeyId::Passkey(passkey.clone())).await;

    let message = create_execute_message(
        &secret_key,
        parameters.next_nonce(),
        market.clone(),
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

    common::assert_tx_succeeded(&harness.network, response.transaction_hash, &ua_account_id)
        .await
        .unwrap();

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

    let SimpleResponse::Success(_result) = response else {
        panic!("Should have succeeded: {response:?}");
    };
}

#[rstest]
#[tokio::test]
pub async fn universal_account_reflexive(#[future(awt)] init_test_owned: InitTest) {
    let InitTest {
        harness,
        app,
        ua_registry,
        borrow_user,
        ..
    } = init_test_owned;

    let (ua_account_id, secret_key, passkey) =
        create_universal_account(&app, &harness.network, &ua_registry, &borrow_user).await;

    // Send an action to the universal account contract

    let load_parameters = async |account_id: AccountId, key: KeyId| {
        templar_relayer::route::universal_account::relay::load_ua_key(&app, account_id, key)
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

    common::assert_tx_succeeded(&harness.network, response.transaction_hash, &ua_account_id)
        .await
        .unwrap();

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
