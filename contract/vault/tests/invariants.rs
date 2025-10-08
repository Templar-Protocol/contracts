// TODO(unit): single-op state machine, all mutators must be idle
// TODO(prop): every callback must be for the current op and market index

// Allocations
// TODO(unit?): on allocation-failure, reconcile to idle
// TODO: allocation accounting: Accepted amount = new_principal - before &never more than attempted
// TODO: allocation attempts: any market that is enabled (new_principal > 0) must be in the withdraw queue

// Withdraws
// TODO: try withdraw & idle first: idle balance can be utilised on a first-come-first-serve basis => it
// is **not** deducted until payout succeeds
// TODO: create withdraw: if create withdraw fails, skip to next market
// TODO: execute withdraw: if executing a withdrawal fails, assume nothing changed
// TODO: withdrawn(execute > read): withdrawn credits must increase idle balance
// TODO: withdraw queue must never have duplicates
// TODO: enabling a market (cap > 0) must add it to the withdraw queue

// TODO: Skim: is no-op when balance is 0

// Payouts
// TODO: payout success: idle balance must decrease & burn escrowed shares
// TODO: payout failure: idle doesnt change  & refund escrowed shares to original owner

// TODO: stop and exit: must never mutiny funds or escrow

// TODO: credit principal only after proper supply to marfket

// TODO: Withdraw read onlky credits idle

// TODO: on error, assume no risk
//

use near_sdk::{json_types::U128, AccountId};
use templar_common::{interest_rate_strategy::InterestRateStrategy, number::Decimal};
use test_utils::{
    controller::vault::UnifiedVaultController, setup_test, setup_test_w, ContractController,
    MarketController, UnifiedMarketController,
};

#[tokio::test]
async fn supply_queue_mustnt_have_duplicates() {}

#[tokio::test]
async fn withdraw_queue_mustnt_have_duplicates() {}

#[tokio::test]
async fn fee_accrual_only_when_aum_grows() {}

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
