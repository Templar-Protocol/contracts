#![allow(clippy::unwrap_used)]
mod common;

use common::{setup_ctx, signer_args};
use near_contract_standards::storage_management::StorageBalance;
use near_sdk::{json_types::U128, serde_json::json, NearToken};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_manager::commands::{
    recover_nep141::RecoverNep141,
    storage_deposit::{StorageDeposit, STORAGE_DEPOSIT_AMOUNT},
};
use test_utils::{
    accounts,
    controller::{ft::FtController, storage_management::StorageManagementController},
    worker, ContractController,
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[rstest]
#[tokio::test]
async fn storage_deposit(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, ft_account, user);

    // Deploy a mock FT contract via test-utils.
    let ft = FtController::deploy(ft_account, "Test Token", "TT").await;

    StorageDeposit {
        signer: signer_args(&user),
        contract_id: ft.id().clone(),
        deposit: None,
        registration_only: false,
    }
    .run(&ctx)
    .await
    .unwrap();

    let balance: StorageBalance = ctx
        .near
        .view(ft.id(), "storage_balance_of")
        .args_json(json!({ "account_id": user.id() }))
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(balance.total, STORAGE_DEPOSIT_AMOUNT);
}

#[rstest]
#[tokio::test]
async fn recover_nep141(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(true, false)] force: bool,
) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, ft_account, source, beneficiary);

    // Deploy FT and set up accounts.
    let ft = FtController::deploy(ft_account, "Test Token", "TT").await;

    // Register both accounts for storage and mint tokens to source.
    ft.storage_deposit(&source, NearToken::from_millinear(100))
        .await;
    ft.storage_deposit(&beneficiary, NearToken::from_millinear(100))
        .await;
    ft.mint(&source, U128(1_000_000)).await;

    // Verify source has tokens.
    let balance: U128 = ctx
        .near
        .view(ft.id(), "ft_balance_of")
        .args_json(json!({ "account_id": source.id() }))
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(balance.0, 1_000_000);

    RecoverNep141 {
        signer: signer_args(&source),
        token_id: ft.id().clone(),
        beneficiary_id: beneficiary.id().clone(),
        force,
    }
    .run(&ctx)
    .await
    .unwrap();

    let beneficiary_balance: U128 = ctx
        .near
        .view(ft.id(), "ft_balance_of")
        .args_json(json!({ "account_id": beneficiary.id() }))
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(beneficiary_balance.0, 1_000_000);

    let source_storage_balance: Option<StorageBalance> = ctx
        .near
        .view(ft.id(), "storage_balance_of")
        .args_json(json!({ "account_id": source.id() }))
        .await
        .unwrap()
        .json()
        .unwrap();
    assert!(source_storage_balance.is_none());
}
