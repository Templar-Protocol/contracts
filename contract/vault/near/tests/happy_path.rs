//! `contract/vault/near/tests/happy_path.rs` ported onto the in-process gateway
//! [`SandboxHarness`]. Every vault interaction goes through the gateway `Client`
//! (via the `vault_*` harness wrappers), the same path the services use — except
//! the one deliberately-invalid atomic batch in
//! [`state_machine_is_locked_when_another_op_is_running`], which has no
//! legitimate gateway op and so uses `near-api` directly.
//!
//! Node-backed: run with `just test-sandbox -p templar-vault-contract`.
#![allow(clippy::too_many_lines)]

use anyhow::Result;
use near_sdk::json_types::U128;
use rstest::rstest;
use templar_common::vault::prelude::{Wad, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD};
use templar_common::vault::{AllocationDelta, Delta};
use templar_gateway_testing::{harness, ManagedAccountId, SandboxHarness};
use templar_primitives::SU128;

mod common;
use common::{harvest, zero_interest};

#[rstest]
#[tokio::test]
async fn donation_does_not_change_aum_until_resync(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let vault = harness
        .deploy_vault_with_market_with(zero_interest, |_| {})
        .await?;
    let supply_user = harness.create_user("supply").await?;
    harness.vault_init_account(&supply_user, &vault).await?;

    harness.vault_supply(&supply_user, &vault, 1_000).await?;

    let total_before = harness.vault_total_assets(&vault).await?;
    let idle_before = harness.vault_idle_balance(&vault).await?;

    // Donate raw tokens straight to the vault account (bypasses the deposit path).
    harness
        .ft_transfer(
            &supply_user,
            &vault.market.borrow_ft_id,
            &vault.vault_id,
            123,
        )
        .await?;

    assert_eq!(
        harness.vault_total_assets(&vault).await?,
        total_before,
        "Donation should not change accounting until resync",
    );
    assert_eq!(
        harness.vault_idle_balance(&vault).await?,
        idle_before,
        "Donation should not change idle accounting until resync",
    );

    harness
        .vault_resync_idle_balance(&supply_user, &vault)
        .await?;

    assert_eq!(
        harness.vault_total_assets(&vault).await?,
        total_before + 123,
        "After resync, total assets should include the donation",
    );
    assert_eq!(
        harness.vault_idle_balance(&vault).await?,
        idle_before + 123,
        "After resync, idle balance should include the donation",
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn supply_queue_mustnt_have_duplicates(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness.deploy_vault_with_market().await?;
    let m = vault.market.market_id.clone();

    let err = harness
        .vault_set_supply_queue(&vault.curator, &vault, &[m.clone(), m])
        .await
        .expect_err("duplicate market in supply queue should be rejected");
    assert!(
        err.to_string().contains("Duplicate market"),
        "expected 'Duplicate market', got: {err}"
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn set_fees_rejects_management_fee_above_cap(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let vault = harness.deploy_vault_with_market().await?;

    let mut fees = harness.vault_get_fees(&vault).await?;
    fees.management.fee = SU128::from(MAX_MANAGEMENT_FEE_WAD + 1);

    let err = harness
        .vault_set_fees(&vault.owner, &vault, fees)
        .await
        .expect_err("management fee above cap should be rejected");
    assert!(
        err.to_string().contains("management fee too high"),
        "expected 'management fee too high', got: {err}"
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn set_fees_rejects_performance_fee_above_cap(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let vault = harness.deploy_vault_with_market().await?;

    let mut fees = harness.vault_get_fees(&vault).await?;
    fees.performance.fee = SU128::from(MAX_PERFORMANCE_FEE_WAD + 1);

    let err = harness
        .vault_set_fees(&vault.owner, &vault, fees)
        .await
        .expect_err("performance fee above cap should be rejected");
    assert!(
        err.to_string().contains("performance fee too high"),
        "expected 'performance fee too high', got: {err}"
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn set_fees_accepts_max_total_assets_growth_rate(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let vault = harness.deploy_vault_with_market().await?;

    let mut fees = harness.vault_get_fees(&vault).await?;
    assert_eq!(fees.max_total_assets_growth_rate, None);

    let rate = SU128::from(u128::from(Wad::one() / 5));
    fees.max_total_assets_growth_rate = Some(rate);
    harness.vault_set_fees(&vault.owner, &vault, fees).await?;

    let updated = harness.vault_get_fees(&vault).await?;
    assert_eq!(
        updated.max_total_assets_growth_rate,
        Some(rate),
        "max_total_assets_growth_rate should persist",
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn state_machine_is_locked_when_another_op_is_running(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    use near_api::{
        types::transaction::actions::{Action, FunctionCallAction},
        Signer, Transaction,
    };
    use near_sdk::NearToken;

    let vault = harness.deploy_vault_with_market().await?;
    let supply_user = harness.create_user("supply").await?;
    harness.vault_init_account(&supply_user, &vault).await?;

    harness.vault_supply(&supply_user, &vault, 1).await?;
    let market_id = harness
        .vault_market_id_of(&vault.vault_id, &vault.market.market_id)
        .await?
        .expect("market registered");

    // Deliberately-invalid atomic batch: `resync_idle_balance` starts an op and
    // `allocate` immediately attempts another in the same transaction, tripping
    // the "only one op in flight" invariant. There is no (and should be no)
    // gateway op for this always-invalid composition, so drive it via near-api.
    let signer = Signer::from_secret_key(templar_gateway_testing::test_secret_key()?)?;
    let result = Transaction::construct(vault.owner.0.clone(), vault.vault_id.clone())
        .add_action(Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "resync_idle_balance".to_owned(),
            args: Vec::new(),
            gas: near_sdk::Gas::from_tgas(30),
            deposit: NearToken::from_yoctonear(0),
        })))
        .add_action(Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "allocate".to_owned(),
            args: near_sdk::serde_json::to_vec(&near_sdk::serde_json::json!({
                "delta": AllocationDelta::Supply(Delta::new(market_id, U128(1))),
            }))?,
            gas: near_sdk::Gas::from_tgas(270),
            deposit: NearToken::from_yoctonear(0),
        })))
        .with_signer(signer)
        .send_to(&harness.network)
        .await?;

    assert!(result.is_failure(), "batch transaction should fail");
    let failures = format!("{result:#?}");
    assert!(
        failures.contains("Invariant: Only one op in flight"),
        "expected ensure_idle invariant failure, got: {failures}"
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn happy(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness
        .deploy_vault_with_market_with(zero_interest, |_| {})
        .await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.vault_init_account(&supply_user, &vault).await?;
    harness.vault_init_account(&borrow_user, &vault).await?;

    let v = vault.vault_id.clone();
    let amount: u128 = 1_000;

    assert_eq!(
        harness.vault_total_assets(&vault).await?,
        0,
        "Vault should appropriately track assets"
    );

    harness.vault_supply(&supply_user, &vault, amount).await?;
    harness
        .collateralize(&borrow_user, &vault.market, 2_000)
        .await?;

    let market_id = harness
        .vault_market_id_of(&vault.vault_id, &vault.market.market_id)
        .await?
        .expect("market registered");

    harness
        .vault_allocate(
            &vault.curator,
            &vault,
            AllocationDelta::Supply(Delta::new(market_id, U128(amount))),
        )
        .await?;

    assert_eq!(
        harness
            .ft_balance_of(&vault.market.borrow_ft_id, &vault.vault_id)
            .await?,
        0,
        "Vault should not have any assets leftover after rebalancing 100%"
    );
    assert_eq!(
        harness.vault_total_supply(&vault).await?,
        amount,
        "Vault should have issued shares to the supplier"
    );
    assert_eq!(
        harness.vault_idle_balance(&vault).await?,
        0,
        "Vault should not have idle balance after allocation"
    );
    assert_eq!(
        harness.vault_total_assets(&vault).await?,
        amount,
        "Vault should appropriately track assets"
    );
    assert_eq!(
        harness
            .get_supply_position(&vault.market, &v)
            .await?
            .unwrap()
            .get_deposit()
            .total(),
        amount.into(),
        "Supply position should match amount of tokens supplied to contract",
    );

    harvest(&harness, &vault).await?;

    assert_eq!(
        u128::from(
            harness
                .get_supply_position(&vault.market, &v)
                .await?
                .unwrap()
                .get_deposit()
                .active
        ),
        amount,
        "Supply position should match amount of tokens supplied to contract",
    );

    let balance_before_withdraw = harness
        .ft_balance_of(&vault.market.borrow_ft_id, &supply_user.0)
        .await?;

    harness
        .vault_withdraw(&supply_user, &vault, amount, None)
        .await?;
    harvest(&harness, &vault).await?;

    let mkt = vault.market.market_id.clone();
    harness
        .vault_allocate(
            &vault.curator,
            &vault,
            AllocationDelta::Withdraw(Delta::new(market_id, U128(amount))),
        )
        .await?;

    harness
        .vault_execute_withdrawal(&vault.curator, &vault, &[mkt.clone()])
        .await?;

    let op_id = harness
        .vault_get_withdrawing_op_id(&vault)
        .await?
        .expect("withdrawing op id");
    harness
        .vault_execute_market_withdrawal(&vault.curator, &vault, op_id, market_id, Some(10))
        .await?;

    assert_eq!(
        harness
            .ft_balance_of(&vault.market.borrow_ft_id, &supply_user.0)
            .await?,
        amount + balance_before_withdraw,
        "Supply user should have received their tokens back"
    );
    assert!(
        harness
            .get_supply_position(&vault.market, &v)
            .await?
            .is_none(),
        "Supply position should be closed"
    );

    // Re-register the vault for a fresh supply position, then re-supply and wait.
    harness
        .storage_deposit_min(&ManagedAccountId(v.clone()), &vault.market.market_id)
        .await?;
    harness.vault_supply(&supply_user, &vault, amount).await?;
    harness
        .vault_allocate(
            &vault.curator,
            &vault,
            AllocationDelta::Supply(Delta::new(market_id, U128(amount))),
        )
        .await?;
    harvest(&harness, &vault).await?;

    // --- Allocator-only rebalance withdrawal into idle (no user withdrawal) ---
    let total_before_rebalance = harness.vault_total_assets(&vault).await?;
    assert_eq!(total_before_rebalance, amount);
    assert_eq!(harness.vault_idle_balance(&vault).await?, 0);

    harness
        .vault_allocate(
            &vault.curator,
            &vault,
            AllocationDelta::Withdraw(Delta::new(market_id, U128(amount))),
        )
        .await?;
    harness
        .vault_execute_rebalance_withdrawal(&vault.curator, &vault, &mkt, None)
        .await?;

    assert_eq!(
        harness.vault_total_assets(&vault).await?,
        total_before_rebalance,
        "Rebalance withdrawal must preserve total assets",
    );
    assert_eq!(
        harness.vault_total_supply(&vault).await?,
        amount,
        "Rebalance withdrawal must not mint or burn shares",
    );
    assert_eq!(
        harness.vault_idle_balance(&vault).await?,
        amount,
        "Rebalance withdrawal should move principal back to idle",
    );
    assert!(
        harness.vault_get_withdrawing_op_id(&vault).await?.is_none(),
        "Rebalance withdrawal must not create a user withdrawing op",
    );

    // Re-allocate idle back into the market, then exercise a borrow + withdraw.
    harness
        .vault_allocate(
            &vault.curator,
            &vault,
            AllocationDelta::Supply(Delta::new(market_id, U128(amount))),
        )
        .await?;
    harvest(&harness, &vault).await?;

    let borrowed = amount / 2;
    harness
        .borrow(&borrow_user, &vault.market, borrowed)
        .await?;

    harness
        .vault_withdraw(&supply_user, &vault, amount - borrowed, None)
        .await?;
    harvest(&harness, &vault).await?;

    harness
        .vault_execute_withdrawal(&vault.curator, &vault, &[mkt.clone()])
        .await?;
    let op_id = harness
        .vault_get_withdrawing_op_id(&vault)
        .await?
        .expect("withdrawing op id");
    harness
        .vault_execute_market_withdrawal(&vault.curator, &vault, op_id, market_id, None)
        .await?;
    Ok(())
}

#[rstest]
#[tokio::test]
async fn deposit_allowed_during_withdrawal_op(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let vault = harness
        .deploy_vault_with_market_with(zero_interest, |_| {})
        .await?;
    let supply_user = harness.create_user("supply").await?;
    let second_user = harness.create_user("second").await?;
    harness.vault_init_account(&supply_user, &vault).await?;
    harness.vault_init_account(&second_user, &vault).await?;

    let amount: u128 = 1_000;
    harness.vault_supply(&supply_user, &vault, amount).await?;

    let market_id = harness
        .vault_market_id_of(&vault.vault_id, &vault.market.market_id)
        .await?
        .expect("market registered");
    harness
        .vault_allocate(
            &vault.curator,
            &vault,
            AllocationDelta::Supply(Delta::new(market_id, U128(amount))),
        )
        .await?;
    harvest(&harness, &vault).await?;

    let withdraw_amount: u128 = 400;
    let balance_before_withdraw = harness
        .ft_balance_of(&vault.market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .vault_withdraw(&supply_user, &vault, withdraw_amount, None)
        .await?;
    harvest(&harness, &vault).await?;

    harness
        .vault_allocate(
            &vault.curator,
            &vault,
            AllocationDelta::Withdraw(Delta::new(market_id, U128(withdraw_amount))),
        )
        .await?;
    harness
        .vault_execute_withdrawal(&vault.curator, &vault, &[vault.market.market_id.clone()])
        .await?;

    let op_id_before = harness
        .vault_get_withdrawing_op_id(&vault)
        .await?
        .expect("withdraw op should exist");

    let deposit_amount: u128 = 250;
    let second_before = harness
        .ft_balance_of(&vault.market.borrow_ft_id, &second_user.0)
        .await?;
    harness
        .vault_supply(&second_user, &vault, deposit_amount)
        .await?;
    let second_after = harness
        .ft_balance_of(&vault.market.borrow_ft_id, &second_user.0)
        .await?;
    let transferred = second_before.saturating_sub(second_after);
    assert!(
        transferred <= deposit_amount,
        "Second user should never transfer more than requested",
    );

    let op_id_after = harness
        .vault_get_withdrawing_op_id(&vault)
        .await?
        .expect("withdraw op should remain active");
    assert_eq!(
        op_id_before, op_id_after,
        "Concurrent deposit must not reset withdrawing op"
    );

    let second_shares = harness
        .ft_balance_of(&vault.vault_id, &second_user.0)
        .await?;
    if transferred > 0 {
        assert!(
            second_shares > 0,
            "Deposit during withdrawal should mint shares when assets are accepted",
        );
    }

    harness
        .vault_execute_market_withdrawal(&vault.curator, &vault, op_id_before, market_id, None)
        .await?;

    assert_eq!(
        harness
            .ft_balance_of(&vault.market.borrow_ft_id, &supply_user.0)
            .await?,
        balance_before_withdraw + withdraw_amount,
        "Withdrawer should receive assets after concurrent deposit"
    );
    assert!(
        harness.vault_get_withdrawing_op_id(&vault).await?.is_none(),
        "Withdraw op should complete"
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn partial_withdrawal_when_market_has_insufficient_liquidity(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let vault = harness
        .deploy_vault_with_market_with(zero_interest, |_| {})
        .await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.vault_init_account(&supply_user, &vault).await?;
    harness.vault_init_account(&borrow_user, &vault).await?;

    let deposit_amount: u128 = 1_000;
    harness
        .vault_supply(&supply_user, &vault, deposit_amount)
        .await?;

    let market_id = harness
        .vault_market_id_of(&vault.vault_id, &vault.market.market_id)
        .await?
        .expect("market registered");
    harness
        .vault_allocate(
            &vault.curator,
            &vault,
            AllocationDelta::Supply(Delta::new(market_id, U128(deposit_amount))),
        )
        .await?;
    harvest(&harness, &vault).await?;
    assert_eq!(
        harness.vault_idle_balance(&vault).await?,
        0,
        "All funds in market"
    );

    // Reduce market liquidity: borrower takes 600, leaving ~400 available.
    harness
        .collateralize(&borrow_user, &vault.market, 2_000)
        .await?;
    let borrow_amount: u128 = 600;
    harness
        .borrow(&borrow_user, &vault.market, borrow_amount)
        .await?;

    let balance_before = harness
        .ft_balance_of(&vault.market.borrow_ft_id, &supply_user.0)
        .await?;
    let shares_before = harness
        .ft_balance_of(&vault.vault_id, &supply_user.0)
        .await?;

    harness
        .vault_withdraw(&supply_user, &vault, deposit_amount, None)
        .await?;
    harvest(&harness, &vault).await?;

    let shares_after_request = harness
        .ft_balance_of(&vault.vault_id, &supply_user.0)
        .await?;
    assert_eq!(
        shares_after_request, 0,
        "All shares should be escrowed during withdrawal",
    );

    let available = deposit_amount - borrow_amount; // 400
    harness
        .vault_allocate(
            &vault.curator,
            &vault,
            AllocationDelta::Withdraw(Delta::new(market_id, U128(available))),
        )
        .await?;
    harness
        .vault_execute_withdrawal(&vault.curator, &vault, &[vault.market.market_id.clone()])
        .await?;

    let op_id = harness
        .vault_get_withdrawing_op_id(&vault)
        .await?
        .expect("withdrawing op");
    harness
        .vault_execute_market_withdrawal(&vault.curator, &vault, op_id, market_id, None)
        .await?;

    let balance_after = harness
        .ft_balance_of(&vault.market.borrow_ft_id, &supply_user.0)
        .await?;
    assert_eq!(
        balance_after - balance_before,
        available,
        "User should receive partial payout equal to available market liquidity",
    );
    assert!(
        harness.vault_get_withdrawing_op_id(&vault).await?.is_none(),
        "Vault should return to idle after partial payout",
    );

    let shares_after = harness
        .ft_balance_of(&vault.vault_id, &supply_user.0)
        .await?;
    let expected_refund = shares_before * borrow_amount / deposit_amount;
    assert!(shares_after > 0, "User should have some shares refunded");
    assert_eq!(
        shares_after, expected_refund,
        "Refunded shares should be proportional to the uncollected amount",
    );

    let total_supply = harness.vault_total_supply(&vault).await?;
    let expected_burned = shares_before * available / deposit_amount;
    assert_eq!(
        total_supply,
        shares_before - expected_burned,
        "Total supply should decrease by burned shares (proportional to payout)",
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn unbrick_recovers_stuck_withdrawal(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness
        .deploy_vault_with_market_with(zero_interest, |_| {})
        .await?;
    let supply_user = harness.create_user("supply").await?;
    harness.vault_init_account(&supply_user, &vault).await?;

    let amount: u128 = 1_000;
    harness.vault_supply(&supply_user, &vault, amount).await?;

    let shares_before = harness
        .ft_balance_of(&vault.vault_id, &supply_user.0)
        .await?;
    assert_eq!(
        shares_before, amount,
        "Shares should equal deposited amount (1:1)"
    );

    let market_id = harness
        .vault_market_id_of(&vault.vault_id, &vault.market.market_id)
        .await?
        .expect("market registered");
    harness
        .vault_allocate(
            &vault.curator,
            &vault,
            AllocationDelta::Supply(Delta::new(market_id, U128(amount))),
        )
        .await?;
    harvest(&harness, &vault).await?;

    harness
        .vault_withdraw(&supply_user, &vault, amount, None)
        .await?;
    harvest(&harness, &vault).await?;

    let shares_after_request = harness
        .ft_balance_of(&vault.vault_id, &supply_user.0)
        .await?;
    assert_eq!(shares_after_request, 0, "Shares should be escrowed");

    harness
        .vault_allocate(
            &vault.curator,
            &vault,
            AllocationDelta::Withdraw(Delta::new(market_id, U128(amount))),
        )
        .await?;
    harness
        .vault_execute_withdrawal(&vault.curator, &vault, &[vault.market.market_id.clone()])
        .await?;

    assert!(
        harness.vault_get_withdrawing_op_id(&vault).await?.is_some(),
        "Vault should be in Withdrawing state",
    );

    harness.vault_unbrick(&vault.curator, &vault).await?;

    assert!(
        harness.vault_get_withdrawing_op_id(&vault).await?.is_none(),
        "Vault should return to idle after unbrick",
    );
    let shares_after_unbrick = harness
        .ft_balance_of(&vault.vault_id, &supply_user.0)
        .await?;
    assert_eq!(
        shares_after_unbrick, shares_before,
        "All escrowed shares should be refunded after unbrick",
    );
    assert_eq!(
        harness.vault_total_supply(&vault).await?,
        shares_before,
        "Total supply should be preserved after unbrick (no burn)",
    );
    Ok(())
}
