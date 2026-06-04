#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Group M — SEP-40 adapter ownership + metadata + upgrade surface.

use soroban_sdk::testutils::{Address as _, BytesN as _};
use soroban_sdk::{Address, BytesN, Symbol};
use templar_proxy_oracle_soroban_common::Asset;
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

#[test]
fn owner_can_update_metadata_and_persist_state() {
    let b = Bootstrap::new();
    let new_base = Asset::Other(Symbol::new(&b.env, "EUR"));
    b.adapter.set_metadata(&4_u32, &2_u32, &new_base);

    assert_eq!(b.adapter.decimals(), 4);
    assert_eq!(b.adapter.resolution(), 2);
    assert_eq!(b.adapter.base(), new_base);
    let cfg = b.adapter.config().unwrap();
    assert_eq!(cfg.decimals, 4);
    assert_eq!(cfg.resolution, 2);
}

#[test]
fn metadata_decimals_above_18_is_rejected() {
    let b = Bootstrap::new();
    let new_base = Asset::Other(Symbol::new(&b.env, "EUR"));
    let result = b.adapter.try_set_metadata(&19_u32, &1_u32, &new_base);
    assert!(result.is_err());
}

#[test]
fn metadata_zero_resolution_is_rejected() {
    let b = Bootstrap::new();
    let new_base = Asset::Other(Symbol::new(&b.env, "EUR"));
    let result = b.adapter.try_set_metadata(&8_u32, &0_u32, &new_base);
    assert!(result.is_err());
}

#[test]
fn adapter_upgrade_with_zero_hash_is_rejected() {
    let b = Bootstrap::new();
    let zero = BytesN::<32>::from_array(&b.env, &[0_u8; 32]);
    let result = b.adapter.try_upgrade(&zero, &b.admin);
    assert!(result.is_err());
}

#[test]
fn adapter_upgrade_by_non_owner_is_rejected() {
    let b = Bootstrap::new();
    let rando = Address::generate(&b.env);
    let dummy_hash = BytesN::<32>::random(&b.env);
    let result = b.adapter.try_upgrade(&dummy_hash, &rando);
    assert!(result.is_err());
}

#[test]
fn two_step_adapter_ownership_handoff() {
    let b = Bootstrap::new();
    let new_owner = Address::generate(&b.env);

    // Owner initiates transfer.
    let live_until_ledger = b.env.ledger().max_live_until_ledger();
    b.adapter.transfer_ownership(&new_owner, &live_until_ledger);

    // Owner unchanged until acceptance.
    assert_eq!(b.adapter.get_owner(), Some(b.admin.clone()));

    b.adapter.accept_ownership();
    assert_eq!(b.adapter.get_owner(), Some(new_owner));
}
