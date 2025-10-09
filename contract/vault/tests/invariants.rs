use near_sdk::{json_types::U128, AccountId};
use templar_common::{interest_rate_strategy::InterestRateStrategy, number::Decimal};
use test_utils::{
    controller::vault::UnifiedVaultController, setup_test, setup_test_w, ContractController,
    MarketController, UnifiedMarketController,
};

// TODO(unit?): on allocation-failure, reconcile to idle

// TODO(prop): every callback must be for the current op and market index
// TODO(prop): allocation accounting: Accepted amount = new_principal - before &never more than attempted
// TODO(prop): allocation attempts: any market that is enabled (new_principal > 0) must be in the withdraw queue
// TODO(prop): withdraw queue must never have duplicates
// TODO(prop): enabling a market (cap > 0) must add it to the withdraw queue

// Withdraws
// TODO(integration): try withdraw & idle first: idle balance can be utilised on a first-come-first-serve basis => it
// is **not** deducted until payout succeeds
// TODO(integration): create withdraw: if create withdraw fails, skip to next market
// TODO(integration): execute withdraw: if executing a withdrawal fails, assume nothing changed
// TODO(integration): withdrawn(execute > read): withdrawn credits must increase idle balance

// TODO: Skim: is no-op when balance is 0

// Payouts
// TODO(integration): payout success: idle balance must decrease & burn escrowed shares
// TODO(integration): payout failure: idle doesnt change  & refund escrowed shares to original owner

// TODO(integration): single-op state machine, all mutators must be idle
// TODO(integration): stop and exit: must never mutiny funds or escrow

// Note: happy path?: credit principal only after proper supply to marfket

// TODO: Withdraw read only credits idle

#[tokio::test]
async fn supply_queue_mustnt_have_duplicates() {}

#[tokio::test]
async fn withdraw_queue_mustnt_have_duplicates() {}

#[tokio::test]
async fn fee_accrual_only_when_aum_grows() {}

#[tokio::test]
#[should_panic = "busy"]
async fn state_machine_is_locked_when_another_op_is_running() {
    setup_test!(
        extract(vault, c, vault_curator)
        accounts(supply_user, borrow_user)
    );
    let amount = 1000;
    let m = c.market.contract().id().clone();
    vault.supply(&supply_user, amount).await;

    let queue = vec![m.clone()];
    tokio::join!(
        vault.allocate(&vault_curator, vec![], Some(amount.into())),
        vault.submit_cap(&vault_curator, m.clone(), (amount * 2).into()),
        vault.set_supply_queue(&vault_curator, &queue),
        vault.set_withdraw_queue(&vault_curator, &queue),
        vault.allocate(&vault_curator, vec![], Some(amount.into())),
    );
}

// #[tokio::test]
// async fn happy() {
//     setup_test!(
//         extract(vault, c, vault_curator)
//         accounts(supply_user, borrow_user)
//         config(|c| {
//             c.borrow_interest_rate_strategy =
//                 InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
//         })
//     );
//     vault.init_account(&supply_user).await;
//
//     let v = vault.contract().id();
//     let amount: U128 = 1000.into();
//
//     vault.supply(&supply_user, amount.0).await;
//     c.collateralize(&borrow_user, 2000).await;
//
//     let weights = vec![(c.market.contract().id().clone(), U128(1))];
//     vault
//         .allocate(&vault_curator, weights.clone(), Some(amount))
//         .await;
//
//     assert_eq!(
//         c.borrow_asset.balance_of(vault.contract().id()).await,
//         0,
//         "Vault should not have any assets leftover after rebalancing 100%"
//     );
//     assert_eq!(
//         vault.get_total_supply().await,
//         amount,
//         "Vault should have issued shares to the supplier"
//     );
//     assert_eq!(
//         vault.get_total_assets().await,
//         amount,
//         "Vault should appropriately track assets"
//     );
//     assert_eq!(
//         c.get_supply_position(v)
//             .await
//             .unwrap()
//             .get_deposit()
//             .total(),
//         amount.into(),
//         "Supply position should match amount of tokens supplied to contract",
//     );
//
//     harvest(&c, &vault).await;
//
//     let supply_position = c.get_supply_position(v).await.unwrap();
//
//     assert_eq!(
//         u128::from(supply_position.get_deposit().active),
//         amount.0,
//         "Supply position should match amount of tokens supplied to contract",
//     );
//
//     let user_balance = c.borrow_asset.balance_of(supply_user.id()).await;
//
//     vault.withdraw(&supply_user, amount, None).await;
//
//     assert_eq!(
//         c.borrow_asset.balance_of(supply_user.id()).await,
//         amount.0 + user_balance,
//         "Supply user should have received their tokens back"
//     );
//
//     let supply_position = c.get_supply_position(v).await;
//     assert!(
//         supply_position.is_none(),
//         "Supply position should be closed"
//     );
//
//     c.storage_deposits(vault.contract().as_account()).await;
//
//     // Resupply and wait
//     vault.supply(&supply_user, amount.0).await;
//     // FIXME:Storage issue:         Error: Error { repr: Custom { kind: Execution, error: ActionError(ActionError { index: Some(0), kind: FunctionCallError(ExecutionError("Smart contract panicked: Storage error: Account vault0251007104533-70674114756315 has insufficient balance: 0.005 NEAR available, but attempted to use 0.008 NEAR")) }) } }
//     vault.allocate(&vault_curator, weights, Some(amount)).await;
//     harvest(&c, &vault).await;
//
//     println!(
//         "Balance of the market for the collateral asset: {}",
//         c.borrow_asset.balance_of(c.market.contract().id()).await
//     );
//
//     let borrowed = amount.0 / 2;
//
//     c.borrow(&borrow_user, borrowed).await;
//
//     vault
//         .withdraw(&supply_user, (amount.0 - borrowed).into(), None)
//         .await;
// }
