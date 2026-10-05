// These tests drive entrypoints for the state effects they produce and assert
// the resulting state, so a promise a call hands back is deliberately not
// followed: the check is what the vault recorded, not the receipt.
#![allow(unused_must_use)]
//! Tests for deposits that arrive while intake is closed, and for the only two
//! ways such a holding can leave the vault: a release to its owner, or
//! admission against an accepted epoch settlement.
//!
//! Every test here asserts observable contract state. A deposit that was
//! protected must not appear in share supply, total assets or the idle balance,
//! and a holding must stop being a holding only when it has actually been
//! returned or actually been priced by an accepted settlement.

use crate::convert::account_id_to_address;
use crate::test_utils::{mk, new_test_contract, set_ctx_with_gas, setup_env};
use crate::Contract;
use near_sdk::json_types::{U128, U64};
use near_sdk::{Gas, PromiseResult};
use near_sdk_contract_tools::ft::{Nep141Controller, Nep145};
use near_sdk::AccountId;

/// Storage attachment large enough for the flows that require one.
const ATTACH: u128 = 3_000_000_000_000_000_000_000_000;

/// Opens intake at a stated ceiling and funds the vault with `amount`, so a
/// test starts from a vault that really holds assets and really issued shares.
fn fund(vault_id: &AccountId, c: &mut Contract, depositor: &AccountId, amount: u128) {
    set_ctx_with_gas(
        vault_id,
        &mk(900),
        Some(2_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.open_intake_for_tests(mk(900), amount.saturating_mul(2));
    set_ctx_with_gas(
        vault_id,
        &mk(6),
        Some(2_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    let refund = c.execute_supply(depositor.clone(), underlying(c), amount, 0);
    assert_eq!(refund, 0, "an open deposit must be admitted in full");
}

fn underlying(c: &Contract) -> AccountId {
    c.underlying_asset.contract_id().into()
}

#[test]
fn deposit_after_cutoff_mints_nothing_and_is_recorded_as_held() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    fund(&vault_id, &mut c, &mk(1), 5_000);

    let supply_before = c.get_total_supply();
    let assets_before = c.get_total_assets();
    let idle_before = c.get_idle_balance();
    let depositor_balance = c.balance_of(&mk(4));

    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.begin_epoch_cutoff(U64(3_000));

    set_ctx_with_gas(
        &vault_id,
        &mk(6),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    let refund = c.execute_supply(mk(4), underlying(&c), 1_000, 250);
    assert_eq!(refund, 0, "a protected deposit is held, not returned");

    assert_eq!(
        c.get_total_supply(),
        supply_before,
        "a protected deposit must not mint shares"
    );
    assert_eq!(
        c.get_total_assets(),
        assets_before,
        "a protected deposit must not enter total assets"
    );
    assert_eq!(
        c.get_idle_balance(),
        idle_before,
        "a protected deposit must not enter the idle balance"
    );
    assert_eq!(
        c.balance_of(&mk(4)),
        depositor_balance,
        "the depositor must hold no shares for a protected deposit"
    );
    assert_eq!(
        c.pending_deposit_total(),
        U128(1_000),
        "the held assets must be recorded as a liability"
    );

    let held = c.get_pending_deposits(mk(4));
    assert_eq!(held.len(), 1, "the holding must be visible to its owner");
    assert_eq!(held[0].1, U128(1_000), "the holding must record its assets");
    assert_eq!(held[0].3, U64(1), "the holding must bind its intake epoch");
}

#[test]
#[should_panic(expected = "caller is not the owner of this pending deposit")]
fn only_the_owner_can_release_a_held_deposit() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    fund(&vault_id, &mut c, &mk(1), 5_000);
    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.begin_epoch_cutoff(U64(3_000));
    set_ctx_with_gas(
        &vault_id,
        &mk(6),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.execute_supply(mk(4), underlying(&c), 1_000, 0);

    set_ctx_with_gas(
        &vault_id,
        &mk(5),
        Some(3_100),
        Some(ATTACH),
        Some(Gas::from_tgas(30)),
    );
    c.refund_pending_deposit(U64(1));
}

#[test]
#[should_panic(expected = "Pending deposit refund failed")]
fn a_failed_release_leaves_the_holding_and_its_floor_in_place() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    fund(&vault_id, &mut c, &mk(1), 5_000);
    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.begin_epoch_cutoff(U64(3_000));
    set_ctx_with_gas(
        &vault_id,
        &mk(6),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.execute_supply(mk(4), underlying(&c), 1_000, 0);

    setup_env(
        &vault_id,
        &vault_id,
        vec![near_sdk::PromiseResult::Failed, PromiseResult::Failed],
    );
    c.finish_pending_deposit_refund(U64(1), mk(4), Err(near_sdk::PromiseError::Failed));
}

#[test]
#[should_panic(expected = "Admission rejected: no accepted settlement covers this deposit")]
fn admission_needs_the_settlement_that_covers_the_deposit() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    fund(&vault_id, &mut c, &mk(1), 5_000);
    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.begin_epoch_cutoff(U64(3_000));
    set_ctx_with_gas(
        &vault_id,
        &mk(6),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.execute_supply(mk(4), underlying(&c), 1_000, 250);

    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.admit_pending_deposit(U64(1));
}

#[test]
#[should_panic(expected = "No pending deposit with this id")]
fn admission_without_a_recorded_holding_cannot_mint() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    fund(&vault_id, &mut c, &mk(1), 5_000);
    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.begin_epoch_cutoff(U64(3_000));
    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_001),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.settle_epoch();

    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_001),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.admit_pending_deposit(U64(1));
}

/// Settlement must run while a holding is recorded, and must then admit it at
/// the snapshot price. A rule that refused settlement until every holding was
/// resolved would leave each holding unpriceable forever.
#[test]
fn settlement_runs_with_a_holding_recorded_and_admission_prices_from_the_snapshot() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    fund(&vault_id, &mut c, &mk(1), 5_000);

    let supply_before = c.get_total_supply();
    let assets_before = c.get_total_assets();
    let idle_before = c.get_idle_balance();

    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.begin_epoch_cutoff(U64(3_000));
    set_ctx_with_gas(
        &vault_id,
        &mk(6),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.execute_supply(mk(4), underlying(&c), 1_000, 250);
    assert_eq!(c.pending_deposit_total(), U128(1_000));

    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_001),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.settle_epoch();
    assert_eq!(
        c.pending_deposit_total(),
        U128(1_000),
        "settlement must neither price nor consume the holding"
    );
    assert_eq!(c.get_total_supply(), supply_before);
    assert_eq!(c.get_total_assets(), assets_before);
    assert_eq!(c.get_idle_balance(), idle_before);

    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_001),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.admit_pending_deposit(U64(1));

    let minted = c.balance_of(&mk(4));
    assert!(minted > 0, "the depositor must receive their own shares");
    assert!(
        minted >= 250 && minted <= 1_000,
        "admission must respect the stored floor and the assets held"
    );
    assert_eq!(
        c.pending_deposit_total(),
        U128(0),
        "admission must remove the holding it priced"
    );
    assert!(
        c.get_pending_deposits(mk(4)).is_empty(),
        "an admitted holding must stop being held"
    );
    assert_eq!(
        c.get_total_assets().0,
        assets_before.0 + 1_000,
        "the held assets may be counted exactly once"
    );
    assert_eq!(
        c.get_idle_balance().0,
        idle_before.0 + 1_000,
        "admitted assets enter the idle balance once"
    );
}

#[test]
fn a_recorded_floor_is_stored_with_the_request_and_a_plain_exit_records_none() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    fund(&vault_id, &mut c, &mk(1), 5_000);

    let shares = c.balance_of(&mk(1));
    assert!(shares > 1, "the funded member must hold shares");

    // Escrow is held by the vault's own account, so that account has to be
    // registered before any exit can attach shares to it.
    set_ctx_with_gas(
        &vault_id,
        &mk(1),
        Some(2_400),
        Some(crate::storage_management::yocto_for_ft_account()),
        Some(Gas::from_tgas(300)),
    );
    c.storage_deposit(Some(vault_id.clone()), None);

    set_ctx_with_gas(
        &vault_id,
        &mk(1),
        Some(2_500),
        Some(crate::storage_management::yocto_for_bytes(
            crate::storage_management::storage_bytes_for_pending_withdrawal(),
        )),
        Some(Gas::from_tgas(300)),
    );
    c.redeem_with_min(U128(shares / 2), mk(1), U128(1_234));
    let id = c.queue_tail().saturating_sub(1);
    assert_eq!(
        c.kernel_state_mirror()
            .withdraw_queue
            .get(id)
            .map(|entry| entry.min_assets_out),
        Some(1_234),
        "a stated floor must be stored with the request"
    );

    set_ctx_with_gas(
        &vault_id,
        &mk(1),
        Some(2_600),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.redeem(U128(1), mk(1));
    assert_eq!(
        c.kernel_state_mirror()
            .withdraw_queue
            .get(c.queue_tail().saturating_sub(1))
            .map(|entry| entry.min_assets_out),
        Some(0),
        "the standard exit ABI must record no floor"
    );
}

#[test]
#[should_panic(expected = "Settlement rejected: held deposit assets are not fully recorded")]
fn settlement_refuses_while_held_assets_are_not_fully_recorded() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    fund(&vault_id, &mut c, &mk(1), 5_000);
    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.begin_epoch_cutoff(U64(3_000));
    set_ctx_with_gas(
        &vault_id,
        &mk(6),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.execute_supply(mk(4), underlying(&c), 1_000, 0);
    c.pending_deposit_assets -= 1;

    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_001),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.settle_epoch();
}

#[test]
#[should_panic]
fn admission_requires_the_allocator_role() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    fund(&vault_id, &mut c, &mk(1), 5_000);
    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.begin_epoch_cutoff(U64(3_000));
    set_ctx_with_gas(
        &vault_id,
        &mk(6),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.execute_supply(mk(4), underlying(&c), 1_000, 250);
    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_001),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.settle_epoch();

    set_ctx_with_gas(
        &vault_id,
        &mk(9),
        Some(3_001),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.admit_pending_deposit(U64(1));
}

#[test]
fn a_held_deposit_keeps_its_owner_findable_after_another_operation() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    fund(&vault_id, &mut c, &mk(1), 5_000);
    set_ctx_with_gas(
        &vault_id,
        &mk(2),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.begin_epoch_cutoff(U64(3_000));
    set_ctx_with_gas(
        &vault_id,
        &mk(6),
        Some(3_000),
        Some(ATTACH),
        Some(Gas::from_tgas(300)),
    );
    c.execute_supply(mk(4), underlying(&c), 1_000, 0);

    c.rebuild_live_address_book();
    assert!(
        c.address_book
            .contains_key(&account_id_to_address(&mk(4))),
        "a held deposit must stay releasable to its owner"
    );
    assert_eq!(c.pending_deposit_total(), U128(1_000));
}
