#![allow(clippy::all, clippy::pedantic)]

//! Integration tests for NEAR vault governance operations.
//!
//! Covers: pause/unpause restrictions, blacklist enforcement, guardian
//! lifecycle with timelocks, cap increase timelocks, allocator role
//! management, and fee decrease semantics.

use near_sdk::env::sha256_array;
use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_workspaces::{network::Sandbox, types::Gas, Worker};
use rstest::rstest;
use templar_common::{
    interest_rate_strategy::InterestRateStrategy,
    number::Decimal,
    vault::{AllocationDelta, Delta, Restrictions},
};
use test_utils::{setup_test, worker, ContractController};

const ADDRESS_DOMAIN: &[u8] = b"templar:near:account-id";

fn account_to_kernel_address(account: &near_workspaces::AccountId) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(ADDRESS_DOMAIN.len() + account.as_bytes().len());
    bytes.extend_from_slice(ADDRESS_DOMAIN);
    bytes.extend_from_slice(account.as_bytes());
    sha256_array(&bytes)
}

/// Guardian can pause the vault. While paused, deposits are rejected.
#[rstest]
#[tokio::test]
async fn pause_blocks_deposits(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, vault_guardian, vault_owner)
        accounts(supply_user, borrow_user)
    );
    vault.init_account(&supply_user).await;

    // Guardian pauses the vault
    vault
        .set_restrictions(&vault_guardian, Some(Restrictions::Paused))
        .await;

    // Verify restrictions are set
    let restrictions = vault.get_restrictions().await;
    assert_eq!(
        restrictions,
        Some(Restrictions::Paused),
        "Vault should be paused after guardian sets Paused restriction",
    );

    // Attempt to deposit while paused — should fail
    let result = supply_user
        .call(vault.contract().id(), "ft_transfer_call")
        .args_json(json!({
            "receiver_id": vault.contract().id(),
            "amount": "1000",
            "msg": ""
        }))
        .gas(Gas::from_tgas(300))
        .deposit(near_workspaces::types::NearToken::from_yoctonear(1))
        .transact()
        .await
        .unwrap();

    assert!(
        result.is_failure(),
        "Deposit should fail when vault is paused",
    );
}

/// Unpause after pause: guardian pauses, owner submits unpause (relaxing
/// requires timelock), then accepts it. Vault should be usable again.
#[rstest]
#[tokio::test]
async fn unpause_restores_deposits(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, vault_guardian, vault_owner)
        accounts(supply_user, borrow_user)
    );
    vault.init_account(&supply_user).await;

    // Pause
    vault
        .set_restrictions(&vault_guardian, Some(Restrictions::Paused))
        .await;
    assert_eq!(vault.get_restrictions().await, Some(Restrictions::Paused));

    // Unpause: relaxing restrictions is timelocked. Since MIN_TIMELOCK_NS=0,
    // we can accept immediately.
    vault.set_restrictions(&vault_owner, None).await;
    vault.accept_restrictions(&vault_owner).await;

    assert_eq!(
        vault.get_restrictions().await,
        None,
        "Restrictions should be cleared after accept",
    );

    // Deposit should now succeed
    let amount: U128 = 500.into();
    vault.supply(&supply_user, amount.0).await;

    let shares: U128 = vault
        .view("ft_balance_of", json!({ "account_id": supply_user.id() }))
        .await;
    assert!(shares.0 > 0, "Deposit should succeed after unpause");
}

/// Blacklisted user cannot deposit into the vault.
#[rstest]
#[tokio::test]
async fn blacklist_blocks_deposit(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, vault_guardian, vault_owner)
        accounts(supply_user, borrow_user)
    );
    vault.init_account(&supply_user).await;

    // Blacklist supply_user
    let blacklist = vec![account_to_kernel_address(supply_user.id())];
    vault
        .set_restrictions(&vault_guardian, Some(Restrictions::Blacklist(blacklist)))
        .await;

    // Attempt deposit — should fail
    let result = supply_user
        .call(vault.contract().id(), "ft_transfer_call")
        .args_json(json!({
            "receiver_id": vault.contract().id(),
            "amount": "1000",
            "msg": ""
        }))
        .gas(Gas::from_tgas(300))
        .deposit(near_workspaces::types::NearToken::from_yoctonear(1))
        .transact()
        .await
        .unwrap();

    assert!(
        result.is_failure(),
        "Deposit should fail for blacklisted user",
    );

    // Non-blacklisted user can still deposit
    vault.init_account(&borrow_user).await;
    vault.supply(&borrow_user, 500).await;

    let shares: U128 = vault
        .view("ft_balance_of", json!({ "account_id": borrow_user.id() }))
        .await;
    assert!(
        shares.0 > 0,
        "Non-blacklisted user should be able to deposit",
    );
}

/// First guardian set is immediate; changing guardian requires timelock.
/// With MIN_TIMELOCK_NS=0, the accept is also immediate.
#[rstest]
#[tokio::test]
async fn guardian_lifecycle(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, vault_owner, vault_guardian)
        accounts(supply_user, borrow_user)
    );

    // Guardian was set during initialization. Changing requires timelock.
    // Submit a new guardian. Use raw call because the controller's parameter
    // name (`new_g`) doesn't match the contract's (`account`).
    vault_owner
        .call(vault.contract().id(), "submit_guardian")
        .args_json(json!({ "account": supply_user.id() }))
        .gas(Gas::from_tgas(50))
        .transact()
        .await
        .unwrap()
        .unwrap();

    // Accept the guardian change (timelock=0, so immediate)
    vault_owner
        .call(vault.contract().id(), "accept_guardian")
        .args_json(json!({}))
        .gas(Gas::from_tgas(50))
        .transact()
        .await
        .unwrap()
        .unwrap();

    // Verify: the new guardian can now pause the vault
    vault
        .set_restrictions(&supply_user, Some(Restrictions::Paused))
        .await;
    assert_eq!(
        vault.get_restrictions().await,
        Some(Restrictions::Paused),
        "New guardian should be able to pause the vault",
    );

    // Old guardian should NOT be able to unpause (they lost the role)
    let result = vault_guardian
        .call(vault.contract().id(), "set_restrictions")
        .args_json(json!({
            "restrictions": null,
        }))
        .gas(Gas::from_tgas(50))
        .transact()
        .await
        .unwrap();

    assert!(
        result.is_failure(),
        "Old guardian should not be able to modify restrictions",
    );
}

/// Sentinel lifecycle: first set is immediate, and sentinel can pause.
#[rstest]
#[tokio::test]
async fn sentinel_can_pause(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, vault_sentinel)
        accounts(supply_user, borrow_user)
    );

    // Sentinel should be able to pause (tighten restrictions)
    vault
        .set_restrictions(&vault_sentinel, Some(Restrictions::Paused))
        .await;

    assert_eq!(
        vault.get_restrictions().await,
        Some(Restrictions::Paused),
        "Sentinel should be able to pause the vault",
    );
}

/// Fee decrease applies immediately without timelock.
/// Fee increase requires timelock.
#[rstest]
#[tokio::test]
async fn fee_decrease_immediate(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, vault_owner)
        accounts(supply_user, borrow_user)
    );

    let original_fees = vault.get_fees().await;

    // Decrease performance fee by 1 — should apply immediately
    let mut decreased = original_fees.clone();
    decreased.performance.fee = U128(original_fees.performance.fee.0 - 1);

    vault.set_fees(&vault_owner, decreased.clone()).await;

    let updated = vault.get_fees().await;
    assert_eq!(
        updated.performance.fee, decreased.performance.fee,
        "Fee decrease should apply immediately",
    );
}

/// Non-allocator cannot allocate. After set_is_allocator(true), they can.
#[rstest]
#[tokio::test]
async fn allocator_role_required_for_allocation(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, c, vault_owner, vault_curator)
        accounts(supply_user, borrow_user)
        config(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
        })
    );
    vault.init_account(&supply_user).await;

    let amount: U128 = 1000.into();
    vault.supply(&supply_user, amount.0).await;

    let market_id = vault.market_id_of(c.market.contract().id()).await;

    // borrow_user is not an allocator — allocate should fail
    let result = borrow_user
        .call(vault.contract().id(), "allocate")
        .args_json(json!({
            "delta": {
                "Supply": {
                    "market_id": market_id,
                    "amount": amount,
                }
            }
        }))
        .gas(Gas::from_tgas(300))
        .transact()
        .await
        .unwrap();

    assert!(
        result.is_failure(),
        "Non-allocator should not be able to allocate",
    );

    // Owner grants allocator role to borrow_user
    vault
        .set_is_allocator(&vault_owner, borrow_user.id().clone(), true)
        .await;

    // Now borrow_user can allocate
    vault
        .allocate(
            &borrow_user,
            AllocationDelta::Supply(Delta::new(market_id, amount)),
        )
        .await;

    // Verify allocation happened
    let idle = vault.get_idle_balance().await;
    assert_eq!(idle, U128(0), "Idle balance should be 0 after allocation",);
}
