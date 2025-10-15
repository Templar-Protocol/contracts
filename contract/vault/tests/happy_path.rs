use near_sdk::{json_types::U128, AccountId};
use templar_common::{interest_rate_strategy::InterestRateStrategy, number::Decimal};
use test_utils::{
    controller::vault::UnifiedVaultController, setup_test, ContractController, MarketController,
    UnifiedMarketController,
};

#[tokio::test]
async fn happy() {
    setup_test!(
        extract(vault, c, vault_curator)
        accounts(supply_user, borrow_user)
        config(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
        })
    );
    vault.init_account(&supply_user).await;

    let v = vault.contract().id();
    let amount: U128 = 1000.into();

    vault.supply(&supply_user, amount.0).await;
    c.collateralize(&borrow_user, 2000).await;

    let weights = vec![(c.market.contract().id().clone(), U128(1))];
    vault
        .allocate(&vault_curator, weights.clone(), Some(amount))
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

    let supply_position = c.get_supply_position(v).await.unwrap();

    assert_eq!(
        u128::from(supply_position.get_deposit().active),
        amount.0,
        "Supply position should match amount of tokens supplied to contract",
    );

    let user_balance = c.borrow_asset.balance_of(supply_user.id()).await;

    vault.withdraw(&supply_user, amount, None).await;
    // TODO: assert the user now escrowed their shares
    vault.execute_next_withdrawal(&vault_curator).await;

    assert_eq!(
        c.borrow_asset.balance_of(supply_user.id()).await,
        amount.0 + user_balance,
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
    // FIXME:Storage issue:         Error: Error { repr: Custom { kind: Execution, error: ActionError(ActionError { index: Some(0), kind: FunctionCallError(ExecutionError("Smart contract panicked: Storage error: Account vault0251007104533-70674114756315 has insufficient balance: 0.005 NEAR available, but attempted to use 0.008 NEAR")) }) } }
    vault.allocate(&vault_curator, weights, Some(amount)).await;
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

    vault.execute_next_withdrawal(&vault_curator).await;
}

// FIXME: should also do this in allocate on behalf of the vault?
pub async fn harvest(c: &UnifiedMarketController, vault: &UnifiedVaultController) {
    // Wait for activation.
    while !c
        .get_supply_position(vault.contract().id())
        .await
        .unwrap()
        .get_deposit()
        .incoming
        .is_empty()
    {
        // TODO: should also do this in allocate
        c.harvest_yield(vault.contract().as_account(), None, None)
            .await;
    }
}
