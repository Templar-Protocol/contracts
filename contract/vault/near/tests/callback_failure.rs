//! `contract/vault/near/tests/callback_failure.rs` ported onto the in-process
//! gateway [`SandboxHarness`]. Exercises the vault's async-failure recovery:
//! `unbrick` from stuck allocation/withdrawal states, its no-op on an idle
//! vault, continued usability after recovery, and that a wrong-market withdrawal
//! step is rejected without corrupting state. Every interaction goes through the
//! gateway `Client` via the `vault_*` harness wrappers.
//!
//! Node-backed: run with
//! `just test-sandbox -p templar-vault-contract --test callback_failure`.
#![allow(clippy::too_many_lines)]

use anyhow::Result;
use near_sdk::json_types::U128;
use rstest::rstest;
use templar_common::vault::{AllocationDelta, Delta, MarketId};
use templar_gateway_testing::{harness, SandboxHarness};

mod common;
use common::{harvest, zero_interest};

/// `unbrick` recovers the vault from a stuck allocation. The allocator-driven
/// allocation completes synchronously here, so `unbrick` is effectively a no-op
/// — but total assets and shares must be preserved regardless.
#[rstest]
#[tokio::test]
async fn unbrick_recovers_stuck_allocation(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness
        .deploy_vault_with_market_with(zero_interest, |_| {})
        .await?;
    let supply_user = harness.create_user("supply").await?;
    harness.vault_init_account(&supply_user, &vault).await?;

    let amount: u128 = 1_000;
    harness.vault_supply(&supply_user, &vault, amount).await?;

    let total_assets_before = harness.vault_total_assets(&vault).await?;
    assert_eq!(
        harness.vault_idle_balance(&vault).await?,
        amount,
        "All assets should be idle before allocation",
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

    harness.vault_unbrick(&vault.curator, &vault).await?;

    assert_eq!(
        harness.vault_total_assets(&vault).await?,
        total_assets_before,
        "Total assets should be preserved after unbrick from allocation",
    );
    assert_eq!(
        harness
            .ft_balance_of(&vault.vault_id, &supply_user.0)
            .await?,
        amount,
        "User shares should be unchanged after unbrick from allocation",
    );
    Ok(())
}

/// `unbrick` on an already-idle vault is a no-op.
#[rstest]
#[tokio::test]
async fn unbrick_noop_when_idle(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness.deploy_vault_with_market().await?;
    let supply_user = harness.create_user("supply").await?;
    harness.vault_init_account(&supply_user, &vault).await?;

    harness.vault_supply(&supply_user, &vault, 500).await?;

    let total_assets_before = harness.vault_total_assets(&vault).await?;
    let total_supply_before = harness.vault_total_supply(&vault).await?;

    harness.vault_unbrick(&vault.curator, &vault).await?;

    assert_eq!(
        harness.vault_total_assets(&vault).await?,
        total_assets_before,
        "Total assets should be unchanged after unbrick from idle",
    );
    assert_eq!(
        harness.vault_total_supply(&vault).await?,
        total_supply_before,
        "Total supply should be unchanged after unbrick from idle",
    );
    Ok(())
}

/// The full recovery cycle: supply → allocate → withdraw → start withdrawal →
/// `unbrick` from Withdrawing, and the vault remains usable for new deposits.
#[rstest]
#[tokio::test]
async fn vault_usable_after_unbrick_recovery(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness
        .deploy_vault_with_market_with(zero_interest, |_| {})
        .await?;
    let supply_user = harness.create_user("supply").await?;
    let second_user = harness.create_user("second").await?;
    harness.vault_init_account(&supply_user, &vault).await?;

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

    harness
        .vault_withdraw(&supply_user, &vault, amount, None)
        .await?;
    harvest(&harness, &vault).await?;

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

    harness.vault_unbrick(&vault.curator, &vault).await?;
    assert!(
        harness.vault_get_withdrawing_op_id(&vault).await?.is_none(),
        "Vault should be idle after unbrick",
    );

    // The vault is still usable — a second user can supply.
    harness.vault_init_account(&second_user, &vault).await?;
    harness.vault_supply(&second_user, &vault, 200).await?;
    assert!(
        harness
            .ft_balance_of(&vault.vault_id, &second_user.0)
            .await?
            > 0,
        "New deposits should work after unbrick recovery",
    );
    Ok(())
}

/// Executing a market-withdrawal step with the wrong `MarketId` is rejected: the
/// vault stops the withdrawal, refunds the escrowed shares, and returns to Idle —
/// a graceful recovery, no `unbrick` needed.
#[rstest]
#[tokio::test]
async fn execute_withdrawal_wrong_market_does_not_corrupt(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let vault = harness
        .deploy_vault_with_market_with(zero_interest, |_| {})
        .await?;
    let supply_user = harness.create_user("supply").await?;
    harness.vault_init_account(&supply_user, &vault).await?;

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

    harness
        .vault_withdraw(&supply_user, &vault, amount, None)
        .await?;
    harvest(&harness, &vault).await?;

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

    let op_id = harness
        .vault_get_withdrawing_op_id(&vault)
        .await?
        .expect("should be in Withdrawing state");

    // A market id with no pending withdrawal. Rather than corrupt state, the
    // vault stops the withdrawal (a `withdrawal_stopped` event with reason
    // "missing market"), refunds the escrowed shares, and returns to Idle — a
    // defined graceful recovery, so no `unbrick` is needed.
    let wrong_market_id = MarketId(market_id.0 + 1);
    harness
        .vault_execute_market_withdrawal(&vault.curator, &vault, op_id, wrong_market_id, None)
        .await?;

    assert!(
        harness.vault_get_withdrawing_op_id(&vault).await?.is_none(),
        "Vault should return to idle after the withdrawal is stopped for a missing market",
    );
    assert_eq!(
        harness
            .ft_balance_of(&vault.vault_id, &supply_user.0)
            .await?,
        amount,
        "Escrowed shares should be refunded when the withdrawal is stopped",
    );
    assert_eq!(
        harness.vault_total_supply(&vault).await?,
        amount,
        "Total supply should be preserved (no shares burned) when nothing is collected",
    );
    Ok(())
}
