//! `contract/vault/near/tests/governance.rs` ported onto the in-process gateway
//! [`SandboxHarness`]. Covers pause/unpause restrictions, blacklist enforcement,
//! sentinel lifecycle timelocks, fee-decrease semantics, and allocator-role
//! gating — every vault interaction through the gateway `Client` (via the
//! `vault_*` harness wrappers), the same path the services use.
//!
//! Node-backed, so gated behind `#[ignore]`; run with:
//! `cargo nextest run -p templar-vault-contract --test governance --run-ignored all`
#![allow(clippy::too_many_lines)]

use anyhow::Result;
use near_sdk::env::sha256_array;
use rstest::rstest;
use templar_common::{
    interest_rate_strategy::InterestRateStrategy,
    vault::{AllocationDelta, Delta, Restrictions},
    Decimal,
};
use templar_gateway_testing::{harness, SandboxHarness};
use templar_primitives::SU128;
use templar_vault_kernel::Address;

/// Domain-separated sha256 of an account id — the canonical kernel address the
/// vault derives internally (mirrors the contract's `pub(crate)`
/// `convert::account_id_to_address`, unreachable from this test crate).
const ADDRESS_DOMAIN: &[u8] = b"templar:near:account-id";
fn account_to_kernel_address(account: &near_api::types::AccountId) -> Address {
    let account = account.as_str().as_bytes();
    let mut bytes = Vec::with_capacity(ADDRESS_DOMAIN.len() + account.len());
    bytes.extend_from_slice(ADDRESS_DOMAIN);
    bytes.extend_from_slice(account);
    Address(sha256_array(&bytes))
}

/// Zero-interest borrow strategy — the market customization the allocation test uses.
fn zero_interest(c: &mut templar_common::market::MarketConfiguration) {
    c.borrow_interest_rate_strategy =
        InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
}

/// Sentinel can pause the vault; while paused, deposits are rejected.
#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn pause_blocks_deposits(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness.deploy_vault_with_market().await?;
    let supply_user = harness.create_user("supply").await?;
    harness.vault_init_account(&supply_user, &vault).await?;

    harness
        .vault_set_restrictions(&vault.sentinel, &vault, Some(Restrictions::Paused))
        .await?;
    assert!(
        matches!(
            harness.vault_get_restrictions(&vault).await?,
            Some(Restrictions::Paused)
        ),
        "Vault should be paused after sentinel sets Paused restriction",
    );

    // The deposit is an `ft_transfer_call`: a paused vault rejects it in
    // `ft_on_transfer` and the FT refunds, but the *top-level* transfer still
    // succeeds, so the gateway reports the op as succeeded (per-receipt failure
    // is invisible to it — ENG-407). Assert on the effect instead: no shares
    // minted and the deposit fully refunded.
    let balance_before = harness
        .ft_balance_of(&vault.market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .try_vault_supply(&supply_user, &vault, 1_000)
        .await?;
    assert_eq!(
        harness
            .ft_balance_of(&vault.vault_id, &supply_user.0)
            .await?,
        0,
        "Paused vault must not mint shares",
    );
    assert_eq!(
        harness
            .ft_balance_of(&vault.market.borrow_ft_id, &supply_user.0)
            .await?,
        balance_before,
        "Paused deposit must be fully refunded",
    );
    Ok(())
}

/// Unpause: sentinel pauses, owner submits+accepts the (timelocked) relaxation,
/// then the vault is usable again.
#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn unpause_restores_deposits(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness.deploy_vault_with_market().await?;
    let supply_user = harness.create_user("supply").await?;
    harness.vault_init_account(&supply_user, &vault).await?;

    harness
        .vault_set_restrictions(&vault.sentinel, &vault, Some(Restrictions::Paused))
        .await?;
    assert!(matches!(
        harness.vault_get_restrictions(&vault).await?,
        Some(Restrictions::Paused)
    ));

    // Relaxing restrictions is timelocked; with MIN_TIMELOCK_NS=0 we accept immediately.
    harness
        .vault_set_restrictions(&vault.owner, &vault, None)
        .await?;
    harness
        .vault_accept_restrictions(&vault.owner, &vault)
        .await?;
    assert!(
        harness.vault_get_restrictions(&vault).await?.is_none(),
        "Restrictions should be cleared after accept",
    );

    harness.vault_supply(&supply_user, &vault, 500).await?;
    let shares = harness
        .ft_balance_of(&vault.vault_id, &supply_user.0)
        .await?;
    assert!(shares > 0, "Deposit should succeed after unpause");
    Ok(())
}

/// A blacklisted user cannot deposit; a non-blacklisted user still can.
#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn blacklist_blocks_deposit(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness.deploy_vault_with_market().await?;
    let supply_user = harness.create_user("supply").await?;
    let other_user = harness.create_user("other").await?;
    harness.vault_init_account(&supply_user, &vault).await?;

    let blacklist = vec![account_to_kernel_address(&supply_user.0)];
    harness
        .vault_set_restrictions(
            &vault.sentinel,
            &vault,
            Some(Restrictions::Blacklist(blacklist)),
        )
        .await?;

    // As in `pause_blocks_deposits`, the rejected deposit refunds under a
    // top-level-successful transfer, so assert on the effect: no shares minted.
    let balance_before = harness
        .ft_balance_of(&vault.market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .try_vault_supply(&supply_user, &vault, 1_000)
        .await?;
    assert_eq!(
        harness
            .ft_balance_of(&vault.vault_id, &supply_user.0)
            .await?,
        0,
        "Blacklisted depositor must not mint shares",
    );
    assert_eq!(
        harness
            .ft_balance_of(&vault.market.borrow_ft_id, &supply_user.0)
            .await?,
        balance_before,
        "Blacklisted deposit must be fully refunded",
    );

    harness.vault_init_account(&other_user, &vault).await?;
    harness.vault_supply(&other_user, &vault, 500).await?;
    let shares = harness
        .ft_balance_of(&vault.vault_id, &other_user.0)
        .await?;
    assert!(shares > 0, "Non-blacklisted user should be able to deposit");
    Ok(())
}

/// Sentinel change is timelocked (MIN_TIMELOCK_NS=0 ⇒ immediate accept): the new
/// sentinel can pause, and the old one loses the role.
#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn sentinel_lifecycle(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness.deploy_vault_with_market().await?;
    let new_sentinel = harness.create_user("new-sentinel").await?;

    harness
        .vault_submit_sentinel(&vault.owner, &vault, &new_sentinel.0)
        .await?;
    harness.vault_accept_sentinel(&vault.owner, &vault).await?;

    // The new sentinel can now pause.
    harness
        .vault_set_restrictions(&new_sentinel, &vault, Some(Restrictions::Paused))
        .await?;
    assert!(
        matches!(
            harness.vault_get_restrictions(&vault).await?,
            Some(Restrictions::Paused)
        ),
        "New sentinel should be able to pause the vault",
    );

    // The old sentinel lost the role and can no longer modify restrictions.
    harness
        .vault_set_restrictions(&vault.sentinel, &vault, None)
        .await
        .expect_err("old sentinel should not be able to modify restrictions");
    Ok(())
}

/// The sentinel set at initialization can pause.
#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn sentinel_can_pause(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness.deploy_vault_with_market().await?;

    harness
        .vault_set_restrictions(&vault.sentinel, &vault, Some(Restrictions::Paused))
        .await?;
    assert!(
        matches!(
            harness.vault_get_restrictions(&vault).await?,
            Some(Restrictions::Paused)
        ),
        "Sentinel should be able to pause the vault",
    );
    Ok(())
}

/// A fee decrease applies immediately (no timelock).
#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn fee_decrease_immediate(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness.deploy_vault_with_market().await?;

    let original = harness.vault_get_fees(&vault).await?;
    let mut decreased = original.clone();
    decreased.performance.fee = SU128::from(original.performance.fee.0 - 1);

    harness
        .vault_set_fees(&vault.owner, &vault, decreased.clone())
        .await?;

    let updated = harness.vault_get_fees(&vault).await?;
    assert_eq!(
        updated.performance.fee, decreased.performance.fee,
        "Fee decrease should apply immediately",
    );
    Ok(())
}

/// A non-allocator cannot allocate; granting the role lets them.
#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn allocator_role_required_for_allocation(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let vault = harness
        .deploy_vault_with_market_with(zero_interest, |_| {})
        .await?;
    let supply_user = harness.create_user("supply").await?;
    let allocator = harness.create_user("allocator").await?;
    harness.vault_init_account(&supply_user, &vault).await?;

    let amount: u128 = 1_000;
    harness.vault_supply(&supply_user, &vault, amount).await?;

    let market_id = harness
        .vault_market_id_of(&vault.vault_id, &vault.market.market_id)
        .await?
        .expect("market registered");

    // Not yet an allocator — allocation is rejected (synchronous auth panic).
    harness
        .vault_allocate(
            &allocator,
            &vault,
            AllocationDelta::Supply(Delta::new(market_id, near_sdk::json_types::U128(amount))),
        )
        .await
        .expect_err("non-allocator should not be able to allocate");

    // Grant the role, then the same allocation succeeds.
    harness
        .vault_set_is_allocator(&vault.owner, &vault, &allocator.0, true)
        .await?;
    harness
        .vault_allocate(
            &allocator,
            &vault,
            AllocationDelta::Supply(Delta::new(market_id, near_sdk::json_types::U128(amount))),
        )
        .await?;

    assert_eq!(
        harness.vault_idle_balance(&vault).await?,
        0,
        "Idle balance should be 0 after allocation",
    );
    Ok(())
}
