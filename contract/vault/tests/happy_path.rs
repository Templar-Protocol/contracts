#![allow(clippy::all, clippy::pedantic)]

use near_sdk::json_types::U128;
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_common::{
    interest_rate_strategy::InterestRateStrategy,
    number::Decimal,
    vault::{AllocationDelta, Delta},
};
use test_utils::{
    controller::vault::UnifiedVaultController, setup_test, worker, ContractController,
    UnifiedMarketController,
};

#[rstest]
#[tokio::test]
#[should_panic = "Duplicate market"]
async fn supply_queue_mustnt_have_duplicates(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, c, vault_curator)
        accounts(supply_user, borrow_user)
    );
    let m = c.market.contract().id().clone();

    let queue = vec![m.clone(), m.clone()];
    vault.set_supply_queue(&vault_curator, &queue).await;
}

#[rstest]
#[tokio::test]
#[should_panic = "Invariant: Only one op in flight"]
async fn state_machine_is_locked_when_another_op_is_running(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    setup_test!(
        worker
        extract(vault, c, vault_owner)
        accounts(supply_user, borrow_user)
    );
    let amount = 1000;
    vault.supply(&supply_user, amount).await;

    futures::future::select_all(
        (0..100).map(|_| Box::pin(vault.allocate(&vault_owner, vec![], Some(1.into())))),
    )
    .await;
}

#[rstest]
#[tokio::test]
async fn happy(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, c, vault_curator)
        accounts(supply_user, borrow_user)
        config(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
        })
    );
    vault.init_account(&supply_user).await;

    let initial_user_balance = c.borrow_asset.balance_of(supply_user.id()).await;
    println!("Initial supply_user balance: {initial_user_balance}");

    let v = vault.contract().id();
    let amount: U128 = 1000.into();

    assert_eq!(
        vault.get_total_assets().await.0,
        0,
        "Vault should appropriately track assets"
    );

    vault.supply(&supply_user, amount.0).await;
    let after_supply_balance = c.borrow_asset.balance_of(supply_user.id()).await;
    println!("After supply of {}: {}", amount.0, after_supply_balance);
    c.collateralize(&borrow_user, 2000).await;

    vault
        .reallocate(
            &vault_curator,
            AllocationDelta::Supply(Delta::new(c.market.contract().id().clone(), amount)),
        )
        .await;

    assert_eq!(
        c.borrow_asset.balance_of(vault.contract().id()).await,
        0,
        "Vault should not have any assets leftover after rebalancing 100%"
    );
    assert_eq!(
        vault.get_total_supply().await,
        amount,
        "Vault should have issued shares to the supplier"
    );
    assert_eq!(
        vault.get_idle_balance().await.0,
        0,
        "Vault should not have idle balance after allocation"
    );
    assert_eq!(
        vault.get_total_assets().await,
        amount,
        "Vault should appropriately track assets"
    );
    assert_eq!(
        c.get_supply_position(v)
            .await
            .unwrap()
            .get_deposit()
            .total(),
        amount.into(),
        "Supply position should match amount of tokens supplied to contract",
    );

    harvest(&c, &vault).await;

    assert_eq!(
        u128::from(c.get_supply_position(v).await.unwrap().get_deposit().active),
        amount.0,
        "Supply position should match amount of tokens supplied to contract",
    );

    let balance_before_withdraw = c.borrow_asset.balance_of(supply_user.id()).await;

    vault.withdraw(&supply_user, amount, None).await;

    harvest(&c, &vault).await;

    let mkt = c.market.contract().id();

    vault
        .reallocate(
            &vault_curator,
            AllocationDelta::Withdraw(Delta::new(mkt.clone(), amount)),
        )
        .await;

    // Plan the withdraw route (single market) and execute it via allocator methods
    let withdraw_route = vec![mkt.clone()];
    vault
        .execute_withdrawal(&vault_curator, withdraw_route.clone())
        .await;

    let op_id = vault
        .vault
        .get_withdrawing_op_id()
        .await
        .expect("Failed to get withdrawing op id");
    vault
        .execute_market_withdrawal(&vault_curator, op_id, 0, Some(10))
        .await;

    assert_eq!(
        c.borrow_asset.balance_of(supply_user.id()).await,
        amount.0 + balance_before_withdraw,
        "Supply user should have received their tokens back"
    );

    let supply_position = c.get_supply_position(v).await;
    assert!(
        supply_position.is_none(),
        "Supply position should be closed"
    );

    c.storage_deposits(vault.contract().as_account()).await;

    // Resupply and wait
    vault.supply(&supply_user, amount.0).await;
    vault
        .reallocate(
            &vault_curator,
            AllocationDelta::Supply(Delta::new(c.market.contract().id().clone(), amount)),
        )
        .await;
    harvest(&c, &vault).await;

    println!(
        "Balance of the market for the collateral asset: {}",
        c.borrow_asset.balance_of(c.market.contract().id()).await
    );

    let borrowed = amount.0 / 2;

    c.borrow(&borrow_user, borrowed).await;

    vault
        .withdraw(&supply_user, (amount.0 - borrowed).into(), None)
        .await;

    // Ensure deposits are activated before we attempt to route and execute the withdrawal
    harvest(&c, &vault).await;
    // Plan the withdraw route (single market) and execute it via allocator methods
    let withdraw_route = vec![c.market.contract().id().clone()];
    vault
        .execute_withdrawal(&vault_curator, withdraw_route.clone())
        .await;
    let op_id = vault
        .vault
        .get_withdrawing_op_id()
        .await
        .expect("Failed to get withdrawing operation ID");
    vault
        .execute_market_withdrawal(&vault_curator, op_id, 0, None)
        .await;
}

pub async fn harvest(c: &UnifiedMarketController, vault: &UnifiedVaultController) {
    // Wait for activation.
    while let Some(position) = c.get_supply_position(vault.contract().id()).await {
        if position.get_deposit().incoming.is_empty() {
            break;
        }
        c.harvest_yield(vault.contract().as_account(), None, None)
            .await;
    }
}
