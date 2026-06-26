//! End-to-end sandbox acceptance test for ENG-369: the liquidator executes a
//! real liquidation against a deployed market through the in-process gateway
//! client.
//!
//! This drives the liquidator's own [`LiquidationExecutor`] (the migrated
//! gateway plan/execute path) — not a re-implementation — against a market with
//! an underwater borrow position, asserting it lands a successful liquidation.
//!
//! Ignored by default: it spins up `near-sandbox` and deploys contracts, so it
//! needs the test wasms prebuilt. Run with:
//!
//! ```bash
//! ./script/prebuild-test-contracts.sh
//! TEST_CONTRACTS_PREBUILT=1 cargo test -p templar-liquidator --test liquidation_sandbox -- --ignored
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use near_sdk::serde_json::{self, json};
use tokio::sync::RwLock;

use templar_common::market::DepositMsg;
use templar_common::oracle::pyth::OracleResponse;
use templar_gateway_client::Client;
use templar_gateway_methods_spec::{market, storage, tx};
use templar_gateway_testing::sandbox::{test_secret_key, SandboxHarness};
use templar_gateway_types::{
    common::ContractArgs, ContractMethodName, ManagedAccountId, NearGas, NearToken, OperationStatus,
};
use templar_liquidator::executor::LiquidationExecutor;
use templar_liquidator::inventory::{InventoryManager, SharedInventory};
use templar_liquidator::swap::SwapRetryConfig;
use templar_liquidator::{CollateralStrategy, LiquidationOutcome};
use test_utils::to_price;

#[tokio::test]
#[ignore = "spins up near-sandbox + deploys contracts; run with --ignored (needs TEST_CONTRACTS_PREBUILT)"]
#[allow(clippy::too_many_lines)]
async fn liquidator_executes_liquidation_on_sandbox() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let (market_id, configuration) = harness.deploy_market().await?;

    let borrow_asset = configuration.borrow_asset.clone();
    let collateral_asset = configuration.collateral_asset.clone();
    let borrow_asset_id = borrow_asset
        .clone()
        .into_nep141()
        .expect("sandbox market uses a NEP-141 borrow asset");
    let collateral_asset_id = collateral_asset
        .clone()
        .into_nep141()
        .expect("sandbox market uses a NEP-141 collateral asset");
    let oracle_cfg = configuration.price_oracle_configuration.clone();

    let liquidator_id = harness.gateway_signer_account_id.clone();
    let borrower_id = harness.cleanup_signer_account_id.clone();

    // Build a gateway client signing for the liquidator, with the borrower also
    // registered so the test can set up its position via `execute_as`. Every
    // harness account shares the fixed test key.
    let key = test_secret_key()?;
    let client = Client::builder(harness.network.clone())
        .secret_key(liquidator_id.clone(), key.clone())?
        .secret_key(borrower_id.clone(), key.clone())?
        .build()?
        .into_signing(liquidator_id.clone())?;

    // Healthy starting prices: borrow $1.00, collateral $2.00.
    harness
        .set_mock_oracle_pyth_price(
            oracle_cfg.account_id.clone(),
            oracle_cfg.borrow_asset_price_id,
            Some(to_price(1.0)),
        )
        .await?;
    harness
        .set_mock_oracle_pyth_price(
            oracle_cfg.account_id.clone(),
            oracle_cfg.collateral_asset_price_id,
            Some(to_price(2.0)),
        )
        .await?;

    // Register both accounts on the assets and the market.
    for account in [&liquidator_id, &borrower_id] {
        for contract_id in [&borrow_asset_id, &collateral_asset_id, &market_id] {
            client
                .execute_as(
                    account.clone(),
                    storage::EnsureDeposit {
                        contract_id: contract_id.clone(),
                        account_id: account.0.clone(),
                        mode: storage::EnsureDepositMode::Registered,
                    },
                )
                .await?;
        }
    }
    // The market itself must hold deposits on both assets to receive tokens via
    // `ft_transfer_call` (collateral in, borrow/liquidation flows out).
    for token in [&borrow_asset_id, &collateral_asset_id] {
        client
            .execute_as(
                liquidator_id.clone(),
                storage::EnsureDeposit {
                    contract_id: token.clone(),
                    account_id: market_id.clone(),
                    mode: storage::EnsureDepositMode::Registered,
                },
            )
            .await?;
    }

    // Mint inventory: the liquidator funds borrow liquidity + liquidation
    // capital; the borrower funds collateral. `mint` credits the predecessor.
    let mint = |account: ManagedAccountId, token: near_account_id::AccountId, amount: &str| {
        let client = client.clone();
        let amount = amount.to_owned();
        async move {
            client
                .execute_as(
                    account,
                    tx::FunctionCall {
                        receiver_id: token,
                        method_name: ContractMethodName("mint".to_owned()),
                        args: ContractArgs::Json(json!({ "amount": amount })),
                        gas: NearGas::from_tgas(100),
                        deposit: NearToken::from_yoctonear(0),
                    },
                )
                .await
        }
    };
    mint(liquidator_id.clone(), borrow_asset_id.clone(), "1000000").await?;
    mint(borrower_id.clone(), collateral_asset_id.clone(), "500000").await?;

    // Liquidator supplies borrow liquidity, then harvests until the supply is active.
    let supply = client
        .execute(market::Supply {
            market_id: market_id.clone(),
            amount: 100_000u128.into(),
        })
        .await?;
    assert_eq!(supply.operation.status, OperationStatus::Succeeded);

    for _ in 0..10 {
        client
            .execute(market::HarvestYield {
                market_id: market_id.clone(),
                account_id: None,
                mode: None,
            })
            .await?;
        let position = client
            .read(market::GetSupplyPosition {
                market_id: market_id.clone(),
                account_id: liquidator_id.0.clone(),
            })
            .await?;
        if position
            .position
            .as_ref()
            .is_some_and(|p| p.get_deposit().incoming.is_empty())
        {
            break;
        }
    }

    // Borrower collateralizes then borrows.
    let collateralize = client
        .execute_as(
            borrower_id.clone(),
            tx::FunctionCall {
                receiver_id: collateral_asset_id.clone(),
                method_name: ContractMethodName("ft_transfer_call".to_owned()),
                args: ContractArgs::Json(json!({
                    "receiver_id": market_id.clone(),
                    "amount": "200000",
                    "msg": serde_json::to_string(&DepositMsg::Collateralize)?,
                })),
                gas: NearGas::from_tgas(300),
                deposit: NearToken::from_yoctonear(1),
            },
        )
        .await?;
    assert_eq!(collateralize.operation.status, OperationStatus::Succeeded);
    let borrow = client
        .execute_as(
            borrower_id.clone(),
            market::Borrow {
                market_id: market_id.clone(),
                amount: 60_000u128.into(),
            },
        )
        .await?;
    assert_eq!(borrow.operation.status, OperationStatus::Succeeded);

    // Crash the collateral price ($2.00 -> $0.05) to push the position underwater.
    harness
        .set_mock_oracle_pyth_price(
            oracle_cfg.account_id.clone(),
            oracle_cfg.collateral_asset_price_id,
            Some(to_price(0.05)),
        )
        .await?;

    // Read the now-liquidatable position and size the liquidation, mirroring the
    // on-chain contract math.
    let position = client
        .read(market::GetBorrowPosition {
            market_id: market_id.clone(),
            account_id: borrower_id.0.clone(),
        })
        .await?
        .position
        .expect("borrower should have a position before liquidation");

    let oracle_response: OracleResponse = HashMap::from([
        (oracle_cfg.borrow_asset_price_id, Some(to_price(1.0))),
        (oracle_cfg.collateral_asset_price_id, Some(to_price(0.05))),
    ]);
    let price_pair = oracle_cfg.create_price_pair(&oracle_response)?;
    let liquidatable_collateral = position.liquidatable_collateral(
        &price_pair,
        configuration.borrow_mcr_maintenance,
        configuration.liquidation_maximum_spread,
    );
    let liquidation_amount = configuration
        .minimum_acceptable_liquidation_amount(liquidatable_collateral, &price_pair)
        .expect("liquidation amount should be derivable for an underwater position");

    // Build the liquidator's inventory (tracks + funds the borrow asset) and executor.
    let mut inventory = InventoryManager::new(client.clone(), liquidator_id.0.clone());
    inventory.discover_assets(std::iter::once(&configuration));
    inventory.refresh_asset(&borrow_asset).await?;
    let inventory: SharedInventory = Arc::new(RwLock::new(inventory));

    let executor = LiquidationExecutor::new(
        client.clone(),
        inventory,
        market_id.clone(),
        false, // dry_run
        CollateralStrategy::Hold,
        None, // swap provider
        SwapRetryConfig::default(),
        0.0, // min swap value (unused for Hold)
        oracle_cfg.collateral_asset_decimals,
    );

    // Execute the liquidation through the liquidator's own gateway path.
    let (outcome, swap_issue) = executor
        .execute_liquidation(
            &borrower_id.0,
            &borrow_asset,
            &collateral_asset,
            liquidation_amount,
            liquidatable_collateral,
            liquidation_amount, // expected collateral value (unused for Hold)
        )
        .await?;

    assert_eq!(outcome, LiquidationOutcome::Liquidated);
    assert!(swap_issue.is_none(), "Hold strategy should not swap");

    Ok(())
}
