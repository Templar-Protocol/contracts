use near_sdk::json_types::U128;
use test_utils::{setup_test, setup_test_w, ContractController};

#[tokio::test]
async fn happy() {
    setup_test!(extract(vault, c, vault_curator) accounts(supply_user ));

    c.init_account(&supply_user).await;
    vault.init_account(&supply_user).await;

    let amount: U128 = 1000.into();

    vault.supply(&supply_user, amount.0).await;

    let weights = vec![(c.market.contract().as_account().id().clone(), U128(1))];
    vault.allocate(&vault_curator, weights, Some(amount)).await;

    assert_eq!(
        c.borrow_asset
            .balance_of(vault.contract().as_account().id())
            .await,
        0
    );
    assert_eq!(vault.get_total_supply().await, amount);
    assert_eq!(vault.get_total_assets().await, amount);
    assert_eq!(
        c.get_supply_position(vault.contract().as_account().id())
            .await
            .unwrap()
            .get_deposit()
            .total(),
        amount.into()
    );
}
