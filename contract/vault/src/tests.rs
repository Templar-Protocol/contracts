use crate::test_utils::*;
use crate::Contract;
use near_sdk::test_utils::accounts;
use near_sdk::{json_types::U128, test_utils::VMContextBuilder, AccountId, RuntimeFeesConfig};
use near_sdk::{test_vm_config, testing_env};
use near_sdk_contract_tools::ft::Nep141Controller as _;
use near_sdk_contract_tools::owner::OwnerExternal;
use rstest::rstest;
use templar_common::asset::{BorrowAsset, FungibleAsset};
use templar_common::vault::MarketConfiguration;
use templar_common::vault::OpState;
use templar_common::vault::{AllocationMode, VaultConfiguration};

#[rstest(len => [2usize, 3, 5])]
#[should_panic = "Duplicate market"]
fn prop_supply_queue_mustnt_have_duplicates(len: usize) {
    let mut c = new_test_contract(&mk(0));
    setup_env(&accounts(0), &accounts(1), vec![]);

    // Build a queue with a duplicate market id
    let base = 100u32;
    let dup = mk(base);
    let mut queue: Vec<AccountId> = Vec::with_capacity(len);
    if len >= 1 {
        queue.push(dup.clone());
    }
    for i in 1..len.saturating_sub(1) {
        queue.push(mk(base + i as u32));
    }
    if len >= 2 {
        queue.push(dup);
    }

    c.set_supply_queue(queue);
}

#[rstest(len => [2usize, 3, 5])]
#[should_panic = "Duplicate market"]
fn prop_withdraw_queue_mustnt_have_duplicates(len: usize) {
    let mut c = new_test_contract(&mk(0));
    setup_env(&accounts(0), &accounts(1), vec![]);

    // Build a queue with a duplicate market id
    let base = 200u32;
    let dup = mk(base);
    let mut queue: Vec<AccountId> = Vec::with_capacity(len);
    if len >= 1 {
        queue.push(dup.clone());
    }
    for i in 1..len.saturating_sub(1) {
        queue.push(mk(base + i as u32));
    }
    if len >= 2 {
        queue.push(dup);
    }

    c.set_withdraw_queue(queue);
}

#[test]
fn fee_accrues_only_on_growth_unit() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Seed total supply so fees can mint
    let user = accounts(1);
    c.deposit_unchecked(&user, 1_000).expect("seed shares");
    c.idle_balance = 1_000;

    // Set fee to 10%
    c.performance_fee = crate::wad::WAD / 10;

    // Baseline: last_total_assets = current, so no profit => no fee
    c.last_total_assets = c.get_total_assets().0;
    let ts_before = c.total_supply();
    c.internal_accrue_fee();
    assert_eq!(c.total_supply(), ts_before, "no profit => no fee minted");

    // Simulate profit: increase idle_balance; now fees should mint
    c.idle_balance = 1_500;
    let expect = crate::wad::compute_fee_shares(
        c.get_total_assets().0,
        c.last_total_assets,
        c.performance_fee,
        c.total_supply(),
    );
    c.internal_accrue_fee();
    assert_eq!(
        c.total_supply(),
        ts_before + expect,
        "fee shares minted must match compute_fee_shares"
    );
}

#[test]
fn contract_convert_roundtrip_bounds() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let c = new_test_contract(&vault_id);

    let a = U128(1_234_567);
    let s = U128(987_654);

    // With virtual offsets, inequalities must hold
    let to_sh = c.convert_to_shares(a);
    let back_a = c.convert_to_assets(to_sh);
    assert!(back_a.0 <= a.0);

    let to_a = c.convert_to_assets(s);
    let back_s = c.convert_to_shares(to_a);
    assert!(back_s.0 >= s.0);
}

#[test]
fn payout_success_burns_only_proportional_escrow_and_refunds_remainder() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let receiver = mk(7);
    let owner = accounts(1);

    // Seed escrow into vault account (shares held by vault)
    c.deposit_unchecked(&near_sdk::env::current_account_id(), 100)
        .expect("seed escrow");
    // Seed idle to cover payout
    c.idle_balance = 1_000;

    // Partial payout scenario: collected/requested = 200/500 => burn 40% of escrowed shares
    c.op_state = OpState::Payout {
        op_id: 1,
        receiver: receiver.clone(),
        amount: 200,
        owner: owner.clone(),
        escrow_shares: 100,
        burn_shares: 40, // precomputed proportional burn for test
    };

    let supply_before = c.total_supply();
    let ok = c.after_send_to_user(Ok(()), 1, receiver, U128(200));
    assert!(ok, "payout must report success");
    // Idle decreased by payout
    assert_eq!(c.idle_balance, 800);
    // Only burn_shares are burned from total supply
    assert_eq!(c.total_supply(), supply_before - 40);
    // State returns to Idle
    assert!(matches!(c.op_state, OpState::Idle));
}

#[test]
fn execute_next_withdrawal_request_skips_holes() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    println!("vault_id: {}", vault_id);
    println!("owner: {}", owner);

    // Bob gets 20 shares
    c.deposit_unchecked(&owner, 20).unwrap();
    // We fake by adding idle to the vault
    c.transfer_unchecked(&owner, &vault_id, 10).unwrap();
    c.transfer_unchecked(&owner, &vault_id, 10).unwrap();

    // Vault now has 20
    assert_eq!(c.balance_of(&vault_id), 20);

    // Queue two requests at ids 1 and 3; head starts at 0
    c.next_withdraw_id = 4;
    c.next_withdraw_to_execute = 0;

    let make = |owner: AccountId, receiver: AccountId| super::PendingWithdrawal {
        owner,
        receiver,
        escrow_shares: 10,
        expected_assets: 5,
        requested_at: 0,
    };
    let recv = mk(9);

    // FIXME: next issue is that we refund if the market doesnt exist and on InsufficientLiquidity
    // balance
    // EVENT_JSON:{"standard":"templar-vault","version":"1.0.0","event":"withdrawal_stopped","data":{"op_id":1,"index":0,"remaining":"5","collected":"0","reason":"InsufficientLiquidity"}}
    // EVENT_JSON:{"standard":"templar-vault","version":"1.0.0","event":"withdrawal_stopped","data":{"op_id":2,"index":0,"remaining":"5","collected":"0","reason":"InsufficientLiquidity"}}

    c.pending_withdrawals
        .insert(1, make(owner.clone(), recv.clone()));
    c.pending_withdrawals
        .insert(3, make(owner.clone(), recv.clone()));

    // First call should consume id=1 and advance head to 2
    let _ = c.execute_next_withdrawal_request();
    assert_eq!(c.next_withdraw_to_execute, 2);

    assert_eq!(c.balance_of(&vault_id), 10);

    // Second call should consume id=3 and advance head to 4
    let _ = c.execute_next_withdrawal_request();
    assert_eq!(c.next_withdraw_to_execute, 4);
}

#[test]
#[should_panic = "unauthorized market"]
fn set_supply_queue_rejects_zero_cap() {
    let mut c = new_test_contract(&mk(0));
    setup_env(&mk(0), &accounts(1), vec![]);

    // Unknown market => cap treated as 0
    c.set_supply_queue(vec![mk(100)]);
}

#[test]
#[should_panic = "Withdraw queue must include all enabled or holding markets"]
fn set_withdraw_queue_must_include_all_holding() {
    let mut c = new_test_contract(&mk(0));
    setup_env(&mk(0), &accounts(1), vec![]);

    let m1 = mk(103);
    let m2 = mk(104);

    // Both known; m1 has supply > 0
    c.config.insert(m1.clone(), MarketConfiguration::default());
    c.config.insert(m2.clone(), MarketConfiguration::default());
    c.market_supply.insert(m1.clone(), 10);

    // Missing m1 should panic
    c.set_withdraw_queue(vec![m2]);
}

#[test]
fn execute_supply_wrong_token_refunds_full() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let sender = accounts(1);
    let wrong_token: AccountId = "wrong.token".parse().unwrap();
    let deposit = 1_000u128;

    let refund = c.execute_supply(sender.clone(), wrong_token.clone(), deposit);
    assert_eq!(refund, deposit, "full refund expected for wrong token");
    assert_eq!(c.total_supply(), 0, "no shares should be minted");
    assert_eq!(c.idle_balance, 0, "idle must remain unchanged");
}

#[test]
#[should_panic = "Withdraw queue must include all enabled or holding markets"]
fn set_withdraw_queue_must_include_all_enabled() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &c.own_get_owner().unwrap(), vec![]);

    let m1 = mk(101);
    let m2 = mk(102);

    // m1 enabled, m2 disabled; provide both configs
    let mut cfg1 = MarketConfiguration::default();
    cfg1.enabled = true;
    c.config.insert(m1.clone(), cfg1);
    c.config.insert(m2.clone(), MarketConfiguration::default());

    // Missing m1 should panic
    c.set_withdraw_queue(vec![m2]);
}
