//! Integration tests for the funding-bridge NEAR treasury handler.
//!
//! These drive the real [`NearHandler`] against the gateway `SandboxHarness`
//! (which deploys a mock FT and provides pre-funded signer accounts) through the
//! in-process gateway client — the same path production uses.
//!
//! Ignored by default: they spin up `near-sandbox` and deploy the mock FT, so
//! they need the test wasms prebuilt. Run with:
//!
//! ```bash
//! ./script/prebuild-test-contracts.sh
//! TEST_CONTRACTS_PREBUILT=1 cargo test -p templar-funding-bridge --test tests -- --ignored
//! ```

#![allow(clippy::unwrap_used)]

use near_account_id::AccountId;
use rstest::{fixture, rstest};
use serde_json::json;

use templar_funding_bridge::{app::App, config::Args, rpc::Network, treasury::NearHandler};
use templar_gateway_client::Client;
use templar_gateway_methods_spec::{ft, storage, tx};
use templar_gateway_testing::sandbox::{test_secret_key, SandboxHarness};
use templar_gateway_types::{
    common::ContractArgs, ContractMethodName, ManagedAccountId, NearGas, NearToken, OperationStatus,
};

/// Amount minted to the treasury in the fixture.
const MINT_AMOUNT: u128 = 1_000_000_000_000;

/// A running sandbox with the mock FT registered for the treasury and user
/// accounts and `MINT_AMOUNT` minted to the treasury, plus a gateway client for
/// reading balances / setting up state.
struct TestContext {
    // Kept alive for the duration of the test (drops the sandbox on teardown).
    _harness: SandboxHarness,
    client: Client,
    treasury: ManagedAccountId,
    user: ManagedAccountId,
    ft: AccountId,
    rpc_url: String,
}

impl TestContext {
    fn treasury_id(&self) -> AccountId {
        self.treasury.0.clone()
    }

    fn user_id(&self) -> AccountId {
        self.user.0.clone()
    }

    /// Read a mock-FT balance through the gateway client.
    async fn ft_balance(&self, account_id: &AccountId) -> u128 {
        *self
            .client
            .read(ft::GetBalanceOf {
                contract_id: self.ft.clone(),
                account_id: account_id.clone(),
            })
            .await
            .unwrap()
            .balance
    }

    /// Build a `NearHandler` for the treasury pointed at the sandbox RPC.
    fn handler(&self, dry_run: bool) -> NearHandler {
        NearHandler::new(
            self.treasury_id(),
            test_secret_key().unwrap(),
            self.rpc_url.clone(),
            Network::Testnet,
            dry_run,
        )
        .unwrap()
    }
}

#[fixture]
async fn ctx() -> TestContext {
    let harness = SandboxHarness::start().await.unwrap();
    let treasury = harness.gateway_signer_account_id.clone();
    let user = harness.cleanup_signer_account_id.clone();
    let ft = harness.ft_contract_id.clone();
    let rpc_url = harness.network.rpc_endpoints[0].url.as_str().to_string();

    // Every harness account shares the fixed test key.
    let key = test_secret_key().unwrap();
    let client = Client::builder(harness.network.clone())
        .secret_key(treasury.clone(), key.clone())
        .unwrap()
        .secret_key(user.clone(), key.clone())
        .unwrap()
        .build()
        .unwrap();

    // Register both accounts for storage on the mock FT.
    for account in [&treasury, &user] {
        client
            .execute_as(
                account.clone(),
                storage::EnsureDeposit {
                    contract_id: ft.clone(),
                    account_id: account.0.clone(),
                    mode: storage::EnsureDepositMode::Registered,
                },
            )
            .await
            .unwrap();
    }

    // Mint to the treasury (mock FT `mint` credits the predecessor). `mint` is a
    // test-only method with no typed op, so this uses `tx::FunctionCall` — the
    // same escape hatch the liquidator sandbox test uses, test-only.
    let result = client
        .execute_as(
            treasury.clone(),
            tx::FunctionCall {
                receiver_id: ft.clone(),
                method_name: ContractMethodName("mint".to_owned()),
                args: ContractArgs::Json(json!({ "amount": MINT_AMOUNT.to_string() })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        result.operation.status,
        OperationStatus::Succeeded,
        "mint should succeed"
    );

    TestContext {
        _harness: harness,
        client,
        treasury,
        user,
        ft,
        rpc_url,
    }
}

fn make_args(ctx: &TestContext) -> Args {
    Args {
        port: 3000,
        network: Network::Testnet,
        bridge_api_url: "https://test.api".to_string(),
        dry_run: false,
        near_treasury_account: Some(ctx.treasury_id()),
        near_treasury_key: Some(test_secret_key().unwrap()),
        near_rpc_url: Some(ctx.rpc_url.clone()),
        eth_private_key: None,
        eth_rpc_url: "https://eth.llamarpc.com".to_string(),
        solana_private_key: None,
        solana_rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
        eth_withdraw_address: Some("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string()),
        arbitrum_withdraw_address: Some("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string()),
        base_withdraw_address: Some("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string()),
        optimism_withdraw_address: None,
        polygon_withdraw_address: None,
        solana_withdraw_address: Some("B4b13ZjqPNGmvK7VVXM3kZ3vEpKS7JVzuqVU6vGqXm9D".to_string()),
        stellar_secret_key: None,
        stellar_horizon_url: "https://horizon.stellar.org".to_string(),
        stellar_withdraw_address: None,
    }
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn test_near_handler_ft_transfer(#[future(awt)] ctx: TestContext) {
    let handler = ctx.handler(false);

    let initial_balance = handler.get_balance(ctx.ft.as_str()).await.unwrap();
    assert_eq!(initial_balance, MINT_AMOUNT);

    let amount = 500_000u128;
    let tx_hash = handler
        .send_tokens(ctx.user_id().as_str(), ctx.ft.as_str(), amount)
        .await
        .unwrap();
    assert!(!tx_hash.is_empty());

    assert_eq!(
        ctx.ft_balance(&ctx.treasury_id()).await,
        MINT_AMOUNT - amount
    );
    assert_eq!(ctx.ft_balance(&ctx.user_id()).await, amount);
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn test_near_handler_dry_run(#[future(awt)] ctx: TestContext) {
    let handler = ctx.handler(true);

    let tx_hash = handler
        .send_tokens(ctx.user_id().as_str(), ctx.ft.as_str(), 500_000)
        .await
        .unwrap();
    assert!(tx_hash.starts_with("dry-run-tx-"));

    // No real transfer happened.
    assert_eq!(ctx.ft_balance(&ctx.user_id()).await, 0);
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn test_near_handler_check_balance(#[future(awt)] ctx: TestContext) {
    let handler = ctx.handler(false);

    let balance = handler.get_balance(ctx.ft.as_str()).await.unwrap();
    assert_eq!(balance, MINT_AMOUNT);
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn test_app_initialization(#[future(awt)] ctx: TestContext) {
    let args = make_args(&ctx);
    let app = App::new(&args).expect("build app");

    assert!(app.is_healthy());
    assert_eq!(
        app.near_handler.treasury_account().as_str(),
        ctx.treasury_id().as_str()
    );
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn test_end_to_end_transfer(#[future(awt)] ctx: TestContext) {
    let handler = ctx.handler(false);

    let tx_hash = handler
        .send_tokens(ctx.user_id().as_str(), ctx.ft.as_str(), 250_000)
        .await
        .unwrap();
    assert!(!tx_hash.is_empty());

    assert_eq!(ctx.ft_balance(&ctx.user_id()).await, 250_000);
}
