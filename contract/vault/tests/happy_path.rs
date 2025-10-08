use near_sdk::json_types::U128;
use test_utils::{setup_test, setup_test_w, ContractController};

#[tokio::test]
async fn happy() {
    setup_test!(extract(vault, c, vault_curator) accounts(supply_user));

    c.init_account(&supply_user).await;
    vault.init_account(&supply_user).await;

    let v = vault.contract().id();
    let amount: U128 = 1000.into();

    vault.supply(&supply_user, amount.0).await;

    let weights = vec![(c.market.contract().id().clone(), U128(1))];
    vault.allocate(&vault_curator, weights, Some(amount)).await;

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

    // Wait for activation.
    while !c
        .get_supply_position(v)
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

    let supply_position = c.get_supply_position(v).await.unwrap();

    assert_eq!(
        u128::from(supply_position.get_deposit().active),
        amount.0,
        "Supply position should match amount of tokens supplied to contract",
    );
}
