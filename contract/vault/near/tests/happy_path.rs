#![allow(clippy::all, clippy::pedantic)]

use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_workspaces::{network::Sandbox, operations::Function, types::Gas, Worker};
use rstest::rstest;
use templar_common::vault::wad::{Wad, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD};
use templar_common::{
    interest_rate_strategy::InterestRateStrategy,
    number::Decimal,
    vault::{AllocationDelta, Delta},
};

#[rstest]
#[tokio::test]
async fn donation_does_not_change_aum_until_resync(#[future(awt)] worker: Worker<Sandbox>) {
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

    let amount: U128 = 1000.into();
    vault.supply(&supply_user, amount.0).await;

    let total_assets_before_donation = vault.get_total_assets().await;
    let idle_before_donation = vault.get_idle_balance().await;

    c.borrow_asset
        .transfer(&supply_user, vault.contract().id(), 123)
        .await;

    assert_eq!(
        vault.get_total_assets().await,
        total_assets_before_donation,
        "Donation should not change accounting until resync",
    );
    assert_eq!(
        vault.get_idle_balance().await,
        idle_before_donation,
        "Donation should not change idle accounting until resync",
    );

    vault.resync_idle_balance(&supply_user).await;

    assert_eq!(
        vault.get_total_assets().await.0,
        total_assets_before_donation.0.saturating_add(123),
        "After resync, total assets should include the donation",
    );
    assert_eq!(
        vault.get_idle_balance().await.0,
        idle_before_donation.0.saturating_add(123),
        "After resync, idle balance should include the donation",
    );
}
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
#[should_panic = "management fee too high"]
async fn set_fees_rejects_management_fee_above_cap(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, vault_owner)
        accounts(supply_user, borrow_user)
    );

    let mut fees = vault.get_fees().await;
    fees.management.fee = U128(MAX_MANAGEMENT_FEE_WAD + 1);

    vault.set_fees(&vault_owner, fees).await;
}

#[rstest]
#[tokio::test]
#[should_panic = "performance fee too high"]
async fn set_fees_rejects_performance_fee_above_cap(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, vault_owner)
        accounts(supply_user, borrow_user)
    );

    let mut fees = vault.get_fees().await;
    fees.performance.fee = U128(MAX_PERFORMANCE_FEE_WAD + 1);

    vault.set_fees(&vault_owner, fees).await;
}

#[rstest]
#[tokio::test]
async fn set_fees_accepts_max_total_assets_growth_rate(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, vault_owner)
        accounts(supply_user, borrow_user)
    );

    let mut fees = vault.get_fees().await;
    assert_eq!(fees.max_total_assets_growth_rate, None);

    fees.max_total_assets_growth_rate = Some(U128(u128::from(Wad::one() / 5)));
    vault.set_fees(&vault_owner, fees.clone()).await;

    let updated = vault.get_fees().await;
    assert_eq!(
        updated.max_total_assets_growth_rate, fees.max_total_assets_growth_rate,
        "max_total_assets_growth_rate should persist",
    );
}

#[rstest]
#[tokio::test]
async fn state_machine_is_locked_when_another_op_is_running(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    setup_test!(
        worker
        extract(vault, c, vault_owner)
        accounts(supply_user, borrow_user)
    );

    vault.supply(&supply_user, 1).await;

    let market_id = vault
        .market_id_of(vault.market.market.contract().id())
        .await;

    let tx = vault_owner
        .batch(vault.contract().id())
        .call(Function::new("resync_idle_balance").gas(Gas::from_tgas(30)))
        .call(
            Function::new("reallocate")
                .args_json(near_sdk::serde_json::json!({
                    "delta": AllocationDelta::Supply(Delta::new(market_id, U128(1))),
                }))
                .gas(Gas::from_tgas(270)),
        )
        .transact()
        .await
        .unwrap();

    assert!(tx.is_failure(), "Batch transaction should fail");

    let failure_text = format!("{:#?}", tx.failures());
    assert!(
        failure_text.contains("Invariant: Only one op in flight"),
        "Expected ensure_idle invariant failure, got failures: {failure_text}"
    );
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

    let market_id = vault.market_id_of(c.market.contract().id()).await;

    vault
        .allocate(
            &vault_curator,
            AllocationDelta::Supply(Delta::new(market_id, amount)),
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
    let mkt_id = vault.market_id_of(mkt).await;

    vault
        .allocate(
            &vault_curator,
            AllocationDelta::Withdraw(Delta::new(mkt_id, amount)),
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
        .execute_market_withdrawal(&vault_curator, op_id, mkt_id, Some(10))
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
    let mkt = c.market.contract().id();
    let mkt_id = vault.market_id_of(mkt).await;
    vault
        .allocate(
            &vault_curator,
            AllocationDelta::Supply(Delta::new(mkt_id, amount)),
        )
        .await;
    harvest(&c, &vault).await;

    // --- Allocator-only rebalance withdrawal into idle (no user withdrawal) ---
    let total_assets_before_rebalance = vault.get_total_assets().await;
    assert_eq!(
        total_assets_before_rebalance, amount,
        "Sanity: total assets should equal supplied amount before rebalance",
    );
    assert_eq!(
        vault.get_idle_balance().await.0,
        0,
        "Idle balance should be zero before rebalance withdrawal",
    );

    // Create a market-side withdrawal request via allocator reallocation.
    vault
        .allocate(
            &vault_curator,
            AllocationDelta::Withdraw(Delta::new(mkt_id, amount)),
        )
        .await;

    // Executing the rebalance withdrawal should pull funds back to idle without
    // touching the user withdrawal queue.
    vault
        .execute_rebalance_withdrawal(&vault_curator, mkt.clone(), None)
        .await;

    assert_eq!(
        vault.get_total_assets().await,
        total_assets_before_rebalance,
        "Rebalance withdrawal must preserve total assets",
    );
    assert_eq!(
        vault.get_total_supply().await,
        amount,
        "Rebalance withdrawal must not mint or burn shares",
    );
    assert_eq!(
        vault.get_idle_balance().await.0,
        amount.0,
        "Rebalance withdrawal should move principal back to idle",
    );
    assert!(
        vault.get_withdrawing_op_id().await.is_none(),
        "Rebalance withdrawal must not create a user withdrawing op",
    );

    // Re-allocate idle back into the market so the later borrow/withdraw path
    // in this test continues to exercise the existing state machine behavior.
    vault
        .allocate(
            &vault_curator,
            AllocationDelta::Supply(Delta::new(mkt_id, amount)),
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
        .execute_market_withdrawal(&vault_curator, op_id, mkt_id, None)
        .await;
}

#[rstest]
#[tokio::test]
async fn deposit_allowed_during_withdrawal_op(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, c, vault_curator)
        accounts(supply_user, second_user)
        config(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
        })
    );
    vault.init_account(&supply_user).await;
    vault.init_account(&second_user).await;

    let amount: U128 = 1000.into();
    vault.supply(&supply_user, amount.0).await;

    let market_id = vault.market_id_of(c.market.contract().id()).await;
    vault
        .allocate(
            &vault_curator,
            AllocationDelta::Supply(Delta::new(market_id, amount)),
        )
        .await;
    harvest(&c, &vault).await;

    let withdraw_amount: U128 = 400.into();
    let balance_before_withdraw = c.borrow_asset.balance_of(supply_user.id()).await;
    vault.withdraw(&supply_user, withdraw_amount, None).await;
    harvest(&c, &vault).await;

    vault
        .allocate(
            &vault_curator,
            AllocationDelta::Withdraw(Delta::new(market_id, withdraw_amount)),
        )
        .await;

    let withdraw_route = vec![c.market.contract().id().clone()];
    vault
        .execute_withdrawal(&vault_curator, withdraw_route)
        .await;

    let op_id_before = vault
        .get_withdrawing_op_id()
        .await
        .expect("withdraw op should exist");

    let deposit_amount: u128 = 250;
    let second_before = c.borrow_asset.balance_of(second_user.id()).await;
    vault.supply(&second_user, deposit_amount).await;
    let second_after = c.borrow_asset.balance_of(second_user.id()).await;
    let transferred = second_before.saturating_sub(second_after);
    assert!(
        transferred <= deposit_amount,
        "Second user should never transfer more than requested",
    );

    let op_id_after = vault
        .get_withdrawing_op_id()
        .await
        .expect("withdraw op should remain active");
    assert_eq!(
        op_id_before, op_id_after,
        "Concurrent deposit must not reset withdrawing op"
    );

    let second_shares: U128 = vault
        .view("ft_balance_of", json!({ "account_id": second_user.id() }))
        .await;
    if transferred > 0 {
        assert!(
            second_shares.0 > 0,
            "Deposit during withdrawal should mint shares when assets are accepted",
        );
    }

    vault
        .execute_market_withdrawal(&vault_curator, op_id_before, market_id, None)
        .await;

    assert_eq!(
        c.borrow_asset.balance_of(supply_user.id()).await,
        balance_before_withdraw + withdraw_amount.0,
        "Withdrawer should receive assets after concurrent deposit"
    );
    assert!(
        vault.get_withdrawing_op_id().await.is_none(),
        "Withdraw op should complete"
    );
}

/// Tests partial withdrawal when market has insufficient liquidity.
///
/// Scenario: user deposits 1000, vault allocates all to market, borrower takes 600.
/// User requests full 1000 withdrawal. The allocator creates a market withdrawal
/// request for only 400 (the available liquidity). The vault collects 400 from the
/// market but the user requested 1000, so the route is exhausted with remaining=600.
/// Verifies that the vault performs a partial payout of 400, burns proportional
/// shares (40%), refunds remaining escrow shares (60%) to the user, and returns
/// to idle.
#[rstest]
#[tokio::test]
async fn partial_withdrawal_when_market_has_insufficient_liquidity(
    #[future(awt)] worker: Worker<Sandbox>,
) {
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

    let deposit_amount: u128 = 1000;
    vault.supply(&supply_user, deposit_amount).await;

    let market_id = vault.market_id_of(c.market.contract().id()).await;

    // Allocate everything to the market
    vault
        .allocate(
            &vault_curator,
            AllocationDelta::Supply(Delta::new(market_id, U128(deposit_amount))),
        )
        .await;
    harvest(&c, &vault).await;

    assert_eq!(
        vault.get_idle_balance().await.0,
        0,
        "All funds should be in the market",
    );

    // Reduce market liquidity: borrower takes 600, leaving ~400 available
    c.collateralize(&borrow_user, 2000).await;
    let borrow_amount: u128 = 600;
    c.borrow(&borrow_user, borrow_amount).await;

    let balance_before = c.borrow_asset.balance_of(supply_user.id()).await;
    let shares_before: U128 = vault
        .view("ft_balance_of", json!({ "account_id": supply_user.id() }))
        .await;

    // User requests full withdrawal of 1000
    vault
        .withdraw(&supply_user, deposit_amount.into(), None)
        .await;
    harvest(&c, &vault).await;

    // Shares should be escrowed (moved to vault contract)
    let shares_after_request: U128 = vault
        .view("ft_balance_of", json!({ "account_id": supply_user.id() }))
        .await;
    assert_eq!(
        shares_after_request.0, 0,
        "All shares should be escrowed during withdrawal",
    );

    // Create market-side withdrawal request for only the available liquidity.
    // The market cannot partially fill a request, so we request only what the
    // market can return (deposit - borrowed = 400).
    let available = deposit_amount - borrow_amount; // 400
    vault
        .allocate(
            &vault_curator,
            AllocationDelta::Withdraw(Delta::new(market_id, U128(available))),
        )
        .await;

    // Execute withdrawal route through the market
    let withdraw_route = vec![c.market.contract().id().clone()];
    vault
        .execute_withdrawal(&vault_curator, withdraw_route)
        .await;

    let op_id = vault
        .get_withdrawing_op_id()
        .await
        .expect("Should have withdrawing op");

    // Execute market withdrawal — market returns 400 (all available liquidity)
    vault
        .execute_market_withdrawal(&vault_curator, op_id, market_id, None)
        .await;

    // --- Assertions ---
    let balance_after = c.borrow_asset.balance_of(supply_user.id()).await;
    let tokens_received = balance_after - balance_before;

    // User should receive the partial payout (~400)
    assert_eq!(
        tokens_received, available,
        "User should receive partial payout equal to available market liquidity",
    );

    // Vault should be back to idle (route exhausted → partial payout → pop head → idle)
    assert!(
        vault.get_withdrawing_op_id().await.is_none(),
        "Vault should return to idle after partial payout",
    );

    // User should have some shares refunded (proportional to uncollected portion)
    let shares_after: U128 = vault
        .view("ft_balance_of", json!({ "account_id": supply_user.id() }))
        .await;
    let expected_refund = shares_before.0 * borrow_amount / deposit_amount; // 600/1000 of original shares
    assert!(
        shares_after.0 > 0,
        "User should have some shares refunded for the uncollected portion",
    );
    assert_eq!(
        shares_after.0, expected_refund,
        "Refunded shares should be proportional to the uncollected amount ({borrow_amount}/{deposit_amount})",
    );

    // Total supply should have decreased by the burned shares (proportional to collected)
    let total_supply = vault.get_total_supply().await;
    let expected_burned = shares_before.0 * available / deposit_amount; // 400/1000 of original shares
    assert_eq!(
        total_supply.0,
        shares_before.0 - expected_burned,
        "Total supply should decrease by burned shares (proportional to payout)",
    );
}

/// Tests that `unbrick` recovers the vault from a stuck Withdrawing state.
///
/// Scenario: user deposits 1000, vault allocates all to market, user requests
/// withdrawal. The vault enters Withdrawing state via `execute_withdrawal`, but
/// instead of completing with `execute_market_withdrawal`, we call `unbrick`.
/// Verifies that escrowed shares are refunded, the queue head is dequeued, and
/// the vault returns to idle.
#[rstest]
#[tokio::test]
async fn unbrick_recovers_stuck_withdrawal(#[future(awt)] worker: Worker<Sandbox>) {
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

    let amount: U128 = 1000.into();
    vault.supply(&supply_user, amount.0).await;

    let shares_before: U128 = vault
        .view("ft_balance_of", json!({ "account_id": supply_user.id() }))
        .await;
    assert_eq!(
        shares_before, amount,
        "Shares should equal deposited amount (1:1)"
    );

    let market_id = vault.market_id_of(c.market.contract().id()).await;

    // Allocate everything to market
    vault
        .allocate(
            &vault_curator,
            AllocationDelta::Supply(Delta::new(market_id, amount)),
        )
        .await;
    harvest(&c, &vault).await;

    // Request full withdrawal
    vault.withdraw(&supply_user, amount, None).await;
    harvest(&c, &vault).await;

    // Shares should be escrowed
    let shares_after_request: U128 = vault
        .view("ft_balance_of", json!({ "account_id": supply_user.id() }))
        .await;
    assert_eq!(shares_after_request.0, 0, "Shares should be escrowed");

    // Create market-side withdrawal request
    vault
        .allocate(
            &vault_curator,
            AllocationDelta::Withdraw(Delta::new(market_id, amount)),
        )
        .await;

    // Start withdrawal execution — vault enters Withdrawing state
    let withdraw_route = vec![c.market.contract().id().clone()];
    vault
        .execute_withdrawal(&vault_curator, withdraw_route)
        .await;

    // Vault should be in Withdrawing state now
    assert!(
        vault.get_withdrawing_op_id().await.is_some(),
        "Vault should be in Withdrawing state",
    );

    // Instead of completing the withdrawal, call unbrick to recover
    vault.unbrick(&vault_curator).await;

    // --- Recovery assertions ---

    // Vault should be back to idle
    assert!(
        vault.get_withdrawing_op_id().await.is_none(),
        "Vault should return to idle after unbrick",
    );

    // Escrowed shares should be refunded to the user
    let shares_after_unbrick: U128 = vault
        .view("ft_balance_of", json!({ "account_id": supply_user.id() }))
        .await;
    assert_eq!(
        shares_after_unbrick, shares_before,
        "All escrowed shares should be refunded after unbrick",
    );

    // Total supply should be preserved (no shares burned or lost)
    let total_supply = vault.get_total_supply().await;
    assert_eq!(
        total_supply, shares_before,
        "Total supply should be preserved after unbrick (no burn)",
    );
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
