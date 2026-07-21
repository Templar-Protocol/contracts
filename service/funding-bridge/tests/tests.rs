//! Integration tests for the funding-bridge NEAR treasury handler.
//!
//! These drive the real [`NearHandler`] against the gateway `SandboxHarness`
//! (which deploys a mock FT and provides pre-funded signer accounts) through the
//! in-process gateway client — the same path production uses.
//!
//! Node-backed: run with `just test-sandbox -p templar-funding-bridge` (which
//! prebuilds the wasms and starts the neard pool).

#![allow(clippy::unwrap_used)]

use near_account_id::AccountId;
use rstest::{fixture, rstest};
use serde_json::json;

use templar_funding_bridge::{rpc::Network, treasury::NearHandler};
use templar_gateway_client::Client;
use templar_gateway_methods_spec::{ft, storage, tx};
use templar_gateway_testing::{
    sandbox::{test_secret_key, SandboxHarness},
    TEST_FINALITY_POLICY,
};
use templar_gateway_types::{
    common::{ContractArgs, TxExecutionStatus},
    ContractMethodName, ManagedAccountId, NearGas, NearToken, OperationStatus,
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
        .finality_policy(TEST_FINALITY_POLICY)
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

    // The handler intentionally retains production-final semantics. Wait once
    // at the setup/handler boundary so its new signer sees the finalized nonce.
    let finalized = client
        .read(tx::Get {
            tx_hash: result
                .operation
                .latest_tx_hash()
                .expect("successful setup transaction should have a hash"),
            sender_account_id: treasury.0.clone(),
            wait_until: Some(TxExecutionStatus::Final),
            encoding: tx::ValueEncoding::Json,
        })
        .await
        .unwrap();
    assert_eq!(finalized.status, tx::Status::Succeeded);

    TestContext {
        _harness: harness,
        client,
        treasury,
        user,
        ft,
        rpc_url,
    }
}

#[rstest]
#[tokio::test]
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

// The dry-run *return value* is covered off-node by
// `treasury::tests::test_send_tokens_dry_run`; this node smoke additionally
// proves the end-to-end safety invariant that a dry-run changes neither the
// treasury nor recipient balance against a deployed FT.
#[rstest]
#[tokio::test]
async fn test_near_handler_dry_run(#[future(awt)] ctx: TestContext) {
    let handler = ctx.handler(true);
    let treasury_before = ctx.ft_balance(&ctx.treasury_id()).await;
    let user_before = ctx.ft_balance(&ctx.user_id()).await;

    let tx_hash = handler
        .send_tokens(ctx.user_id().as_str(), ctx.ft.as_str(), 500_000)
        .await
        .unwrap();
    assert!(tx_hash.starts_with("dry-run-tx-"));

    assert_eq!(ctx.ft_balance(&ctx.treasury_id()).await, treasury_before);
    assert_eq!(ctx.ft_balance(&ctx.user_id()).await, user_before);
}

#[rstest]
#[tokio::test]
async fn test_near_handler_check_balance(#[future(awt)] ctx: TestContext) {
    let handler = ctx.handler(false);

    let balance = handler.get_balance(ctx.ft.as_str()).await.unwrap();
    assert_eq!(balance, MINT_AMOUNT);
}

// App construction from config is covered off-node by
// `app::tests::app_new_builds_configured_treasury_handler`.

#[rstest]
#[tokio::test]
async fn test_end_to_end_transfer(#[future(awt)] ctx: TestContext) {
    let handler = ctx.handler(false);

    let tx_hash = handler
        .send_tokens(ctx.user_id().as_str(), ctx.ft.as_str(), 250_000)
        .await
        .unwrap();
    assert!(!tx_hash.is_empty());

    assert_eq!(ctx.ft_balance(&ctx.user_id()).await, 250_000);
}
