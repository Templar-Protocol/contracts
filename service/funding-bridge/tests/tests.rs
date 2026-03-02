//! Integration tests for funding-bridge service
//!
//! Tests the NEAR chain handler with real NEAR sandbox environment

#![allow(clippy::unwrap_used)]

use near_sdk::{json_types::U128, NearToken};
use near_workspaces::{Account, Contract, Worker};
use std::sync::Arc;

use templar_funding_bridge::{
    app::App, bridge::BridgeClient, config::Args, rpc::Network, treasury::NearHandler,
};

const FT_WASM: &[u8] = include_bytes!("../../../mock/ft/res/mock_ft.wasm");

/// Test fixture containing sandbox worker and accounts
struct TestContext {
    worker: Worker<near_workspaces::network::Sandbox>,
    treasury: Account,
    user: Account,
    ft_contract: Contract,
}

impl TestContext {
    async fn new() -> Self {
        let worker = near_workspaces::sandbox().await.unwrap();

        // Create accounts
        let treasury = worker.dev_create_account().await.unwrap();
        let user = worker.dev_create_account().await.unwrap();

        // Deploy FT contract
        let ft_contract = worker.dev_deploy(FT_WASM).await.unwrap();

        // Initialize FT contract
        ft_contract
            .call("new")
            .args_json(serde_json::json!({
                "name": "Test Token",
                "symbol": "TEST"
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();

        // Register treasury account with storage deposit
        treasury
            .call(ft_contract.id(), "storage_deposit")
            .args_json(serde_json::json!({
                "account_id": treasury.id()
            }))
            .deposit(NearToken::from_millinear(10))
            .transact()
            .await
            .unwrap()
            .unwrap();

        // Register user account with storage deposit
        treasury
            .call(ft_contract.id(), "storage_deposit")
            .args_json(serde_json::json!({
                "account_id": user.id()
            }))
            .deposit(NearToken::from_millinear(10))
            .transact()
            .await
            .unwrap()
            .unwrap();

        // Mint tokens to treasury account
        treasury
            .call(ft_contract.id(), "mint")
            .args_json(serde_json::json!({
                "amount": U128::from(1_000_000_000_000u128)
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();

        // Verify minting worked
        let balance: U128 = ft_contract
            .view("ft_balance_of")
            .args_json(serde_json::json!({
                "account_id": treasury.id()
            }))
            .await
            .unwrap()
            .json()
            .unwrap();

        assert_eq!(
            balance.0, 1_000_000_000_000u128,
            "Treasury should have minted tokens"
        );

        Self {
            worker,
            treasury,
            user,
            ft_contract,
        }
    }

    async fn get_ft_balance(&self, account_id: &near_sdk::AccountId) -> u128 {
        let result: U128 = self
            .ft_contract
            .view("ft_balance_of")
            .args_json(serde_json::json!({
                "account_id": account_id
            }))
            .await
            .unwrap()
            .json()
            .unwrap();

        result.0
    }
}

#[tokio::test]
async fn test_near_handler_ft_transfer() {
    let ctx = TestContext::new().await;

    // Create NEAR handler
    let handler = NearHandler::new(
        ctx.treasury.id().as_str().parse().unwrap(),
        ctx.treasury.secret_key().to_string().parse().unwrap(),
        ctx.worker.rpc_addr(),
        false, // not dry run
    );

    // Check initial balance
    let initial_balance = handler
        .get_balance(ctx.ft_contract.id().as_str())
        .await
        .unwrap();

    assert_eq!(initial_balance, 1_000_000_000_000u128);

    // Transfer tokens to user
    let amount = 500_000u128;
    let tx_hash = handler
        .send_tokens(
            ctx.user.id().as_str(),
            ctx.ft_contract.id().as_str(),
            amount,
        )
        .await
        .unwrap();

    assert!(!tx_hash.is_empty());

    // Verify balances
    let treasury_balance = ctx
        .get_ft_balance(&ctx.treasury.id().as_str().parse().unwrap())
        .await;
    let user_balance = ctx
        .get_ft_balance(&ctx.user.id().as_str().parse().unwrap())
        .await;

    assert_eq!(treasury_balance, 1_000_000_000_000u128 - amount);
    assert_eq!(user_balance, amount);
}

#[tokio::test]
async fn test_near_handler_dry_run() {
    let ctx = TestContext::new().await;

    // Create NEAR handler in dry-run mode
    let handler = NearHandler::new(
        ctx.treasury.id().as_str().parse().unwrap(),
        ctx.treasury.secret_key().to_string().parse().unwrap(),
        ctx.worker.rpc_addr(),
        true, // dry run
    );

    // Transfer should return immediately without actual transaction
    let tx_hash = handler
        .send_tokens(
            ctx.user.id().as_str(),
            ctx.ft_contract.id().as_str(),
            500_000u128,
        )
        .await
        .unwrap();

    assert!(tx_hash.starts_with("dry-run-tx-"));

    // Verify no actual transfer happened
    let user_balance = ctx
        .get_ft_balance(&ctx.user.id().as_str().parse().unwrap())
        .await;

    assert_eq!(user_balance, 0);
}

#[tokio::test]
async fn test_near_handler_check_balance() {
    let ctx = TestContext::new().await;

    // Create handler with real sandbox
    let handler = NearHandler::new(
        ctx.treasury.id().as_str().parse().unwrap(),
        ctx.treasury.secret_key().to_string().parse().unwrap(),
        ctx.worker.rpc_addr(),
        false,
    );

    // Check balance should work
    let balance = handler
        .get_balance(ctx.ft_contract.id().as_str())
        .await
        .unwrap();

    assert_eq!(balance, 1_000_000_000_000u128);
}

#[tokio::test]
async fn test_app_initialization() {
    let ctx = TestContext::new().await;

    // Create minimal config for testing
    let bridge_client = Arc::new(BridgeClient::new("https://test.api".to_string()));
    let token_registry =
        templar_funding_bridge::tokens::TokenRegistry::new(Arc::clone(&bridge_client));

    let near_handler = Arc::new(NearHandler::new(
        ctx.treasury.id().as_str().parse().unwrap(),
        ctx.treasury.secret_key().to_string().parse().unwrap(),
        ctx.worker.rpc_addr(),
        false,
    ));

    let args = Args {
        port: 3000,
        network: Network::Testnet,
        bridge_api_url: "https://test.api".to_string(),
        dry_run: false,
        near_treasury_account: Some(ctx.treasury.id().as_str().parse().unwrap()),
        near_treasury_key: Some(ctx.treasury.secret_key().to_string().parse().unwrap()),
        near_rpc_url: Some(ctx.worker.rpc_addr()),
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
    };

    let app = App {
        near_handler: near_handler.clone(),
        bridge_client,
        token_registry,
        external_chains: std::sync::Arc::new(
            templar_funding_bridge::external::ExternalChainRegistry::new(),
        ),
        bridge_relayer: None,
        bridge_relayer_auth_token: None,
        config: Arc::new(args),
        dry_run: false,
        version: "0.1.0-test",
    };

    // App should be healthy
    assert!(app.is_healthy());

    // Check treasury account
    assert_eq!(
        app.near_handler.treasury_account().as_str(),
        ctx.treasury.id().as_str()
    );
}

#[tokio::test]
async fn test_end_to_end_transfer() {
    let ctx = TestContext::new().await;

    let handler = NearHandler::new(
        ctx.treasury.id().as_str().parse().unwrap(),
        ctx.treasury.secret_key().to_string().parse().unwrap(),
        ctx.worker.rpc_addr(),
        false,
    );

    // Execute transfer directly via NearHandler
    let tx_hash = handler
        .send_tokens(
            ctx.user.id().as_str(),
            ctx.ft_contract.id().as_str(),
            250_000u128,
        )
        .await
        .unwrap();

    assert!(!tx_hash.is_empty());

    // Verify transfer
    let user_balance = ctx
        .get_ft_balance(&ctx.user.id().as_str().parse().unwrap())
        .await;

    assert_eq!(user_balance, 250_000u128);
}
