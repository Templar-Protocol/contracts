use std::u64;

use crate::impl_callbacks::reconcile_supply_outcome;
use crate::impl_callbacks::WithdrawReconciliation;
use crate::storage_management::storage_bytes_for_queue_account_id;
use crate::storage_management::yocto_for_bytes;
use crate::storage_management::yocto_for_new_market;
use crate::storage_management::yocto_for_pending_cap;
use crate::test_utils::*;
use crate::wad::compute_fee_shares;
use crate::Contract;
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver as _;
use near_sdk::env;
use near_sdk::serde_json;
use near_sdk::test_utils::accounts;
use near_sdk::PromiseOrValue;
use near_sdk::PromiseResult;
use near_sdk::{json_types::U128, AccountId};
use near_sdk_contract_tools::ft::Nep141 as _;
use near_sdk_contract_tools::ft::Nep141Controller as _;
use near_sdk_contract_tools::mt::Nep245Receiver as _;
use near_sdk_contract_tools::owner::OwnerExternal;
use rstest::{fixture, rstest};
use templar_common::vault::Error;
use templar_common::vault::MarketConfiguration;
use templar_common::vault::OpState;
use templar_common::vault::{AllocationMode, DepositMsg};

#[fixture]
fn vault_id_fixture() -> AccountId {
    accounts(0)
}

#[fixture]
fn c_vault_env(vault_id_fixture: AccountId) -> Contract {
    setup_env(&vault_id_fixture, &vault_id_fixture, vec![]);
    new_test_contract(&vault_id_fixture)
}

#[fixture]
fn c_owner_env(vault_id_fixture: AccountId) -> Contract {
    let c = new_test_contract(&vault_id_fixture);
    let owner = c
        .own_get_owner()
        .unwrap_or_else(|| env::panic_str(&"Owner not set".to_string()));
    setup_env(&vault_id_fixture, &owner, vec![]);
    c
}

#[fixture]
fn c_asset_env(vault_id_fixture: AccountId) -> Contract {
    let c = new_test_contract(&vault_id_fixture);
    let asset: AccountId = c.underlying_asset.contract_id().into();
    setup_env(&vault_id_fixture, &asset, vec![]);
    c
}

#[fixture]
fn enabled_market_100() -> (AccountId, MarketConfiguration) {
    let m = mk(9001);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(100);
    cfg.enabled = true;
    (m, cfg)
}

#[fixture]
fn vault_id() -> AccountId {
    accounts(0)
}

#[fixture]
fn c(vault_id: AccountId) -> Contract {
    setup_env(&vault_id, &vault_id, vec![]);
    new_test_contract(&vault_id)
}

// Contract with the env used by after_supply_1_check_* tests
#[fixture]
fn c_max(vault_id: AccountId) -> Contract {
    setup_env(
        &vault_id,
        &vault_id,
        vec![PromiseResult::Successful(
            near_sdk::serde_json::to_vec(&U128(u128::MAX))
                .unwrap_or_else(|e| near_sdk::env::panic_str(&e.to_string())),
        )],
    );
    new_test_contract(&vault_id)
}

#[fixture]
fn receiver() -> AccountId {
    mk(9)
}

#[fixture]
fn owner() -> AccountId {
    accounts(1)
}

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

#[rstest]
fn fee_accrues_only_on_growth_unit(mut c_vault_env: Contract) {
    let mut c = c_vault_env;

    // Seed total supply so fees can mint
    let user = accounts(1);
    c.deposit_unchecked(&user, 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    c.idle_balance = 1_000;

    // Set fee to 10%
    c.performance_fee = crate::wad::Wad::one() / 10;

    // Baseline: last_total_assets = current, so no profit => no fee
    c.last_total_assets = c.get_total_assets().0;
    let ts_before = c.total_supply();
    c.internal_accrue_fee();
    assert_eq!(c.total_supply(), ts_before, "no profit => no fee minted");

    // Simulate profit: increase idle_balance; now fees should mint
    c.idle_balance = 1_500;
    let expect = compute_fee_shares(
        c.get_total_assets().0.into(),
        c.last_total_assets.into(),
        c.performance_fee,
        c.total_supply().into(),
    );
    c.internal_accrue_fee();
    assert_eq!(
        c.total_supply(),
        ts_before + expect.as_u128_trunc(),
        "fee shares minted must match compute_fee_shares"
    );
}

#[rstest]
fn payout_success_burns_only_proportional_escrow_and_refunds_remainder(mut c_vault_env: Contract) {
    let mut c = c_vault_env;

    let receiver = mk(7);
    let owner = accounts(1);

    // Seed escrow into vault account (shares held by vault)
    c.deposit_unchecked(&near_sdk::env::current_account_id(), 100)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
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

#[rstest]
fn execute_next_withdrawal_request_skips_holes(mut c_owner_env: Contract) {
    let mut c = c_owner_env;
    let vault_id = accounts(0);
    let owner = c
        .own_get_owner()
        .unwrap_or_else(|| env::panic_str(&"Owner not set".to_string()));

    println!("vault_id: {vault_id}");
    println!("owner: {owner}");

    // Bob gets 20 shares
    c.deposit_unchecked(&owner, 20)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    // We fake by adding idle to the vault
    c.transfer_unchecked(&owner, &vault_id, 10)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    c.transfer_unchecked(&owner, &vault_id, 10)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));

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

#[rstest]
fn execute_supply_wrong_token_refunds_full(mut c_vault_env: Contract) {
    let mut c = c_vault_env;

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
    setup_env(
        &vault_id,
        &c.own_get_owner()
            .unwrap_or_else(|| env::panic_str(&"Owner not set".to_string())),
        vec![],
    );

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

#[rstest]
fn start_allocation_reserves_only_amount(mut c_vault_env: Contract) {
    let mut c = c_vault_env;

    // Configure a single market with cap = 80 in the supply queue
    let m1 = mk(2000);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(80);
    cfg.enabled = true;
    c.config.insert(m1.clone(), cfg);
    c.supply_queue.push(m1.clone());

    // Idle = 100, so max_room (80) should clamp allocation
    c.idle_balance = 100;
    assert_eq!(c.get_max_deposit().0, 80, "sanity: max room must be 80");

    // Reserve only the amount to allocate (intended behavior)
    let total = c.get_max_deposit().0.min(c.idle_balance);
    c.start_allocation(total);

    // Emulate allocation completing successfully: 80 moved to market
    c.market_supply.insert(m1.clone(), 80);
    if !c.withdraw_queue.iter().any(|x| x == &m1) {
        c.withdraw_queue.push(m1.clone());
    }
    // Force completion and exit op
    if let crate::OpState::Allocating { op_id, index, .. } = c.op_state.clone() {
        c.op_state = crate::OpState::Allocating {
            op_id,
            index,
            remaining: 0,
        };
    } else {
        panic!("expected Allocating state");
    }
    let _ = c.stop_and_exit::<str>(None);

    // Expected post-conditions:
    // - idle should retain 20
    // - total assets (idle + market principals) should remain 100
    assert_eq!(
        c.idle_balance, 20,
        "idle should retain unallocated amount (100 - 80)"
    );
    assert_eq!(
        c.get_total_assets().0,
        100,
        "total assets must remain unchanged at 100"
    );
}

#[test]
fn queue_allocation_ignores_stale_plan() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(
        &vault_id,
        &c.own_get_owner()
            .unwrap_or_else(|| env::panic_str(&"Owner not set".to_string())),
        vec![],
    );

    // Supply queue has m1; stale plan points to m2
    let m1 = mk(3001);
    let m2 = mk(3002);

    let mut cfg1 = MarketConfiguration::default();
    cfg1.cap = U128(10);
    cfg1.enabled = true;
    c.config.insert(m1.clone(), cfg1);
    c.withdraw_queue.push(m1.clone());
    c.supply_queue.push(m1);

    // Stale plan (should be ignored for queue-based allocation)
    c.plan = Some(vec![(m2.clone(), 1u128)]);

    c.idle_balance = 5;

    // Run queue-based allocation (weights empty) -> must clear any stale plan
    let weights: templar_common::vault::AllocationWeights = vec![];
    let _ = c.allocate(weights, None);

    assert!(
        c.plan.is_none(),
        "queue-based allocate must ignore and clear any stale plan"
    );
}

#[test]
#[should_panic = "Market still has supply but no removal scheduled"]
fn set_withdraw_queue_disallow_nonzero_position_removal() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(
        &vault_id,
        &c.own_get_owner()
            .unwrap_or_else(|| env::panic_str(&"Owner not set".to_string())),
        vec![],
    );

    let m1 = mk(4001);

    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(0); // required precondition to attempt removal
    cfg.enabled = true;
    c.config.insert(m1.clone(), cfg);

    // Market has non-zero position but no removal scheduled
    c.market_supply.insert(m1.clone(), 1);

    // Present in current withdraw queue so removal logic executes
    c.withdraw_queue.push(m1);

    // Attempting to remove should panic due to non-zero position without removal schedule
    c.set_withdraw_queue(vec![]);
}

#[rstest(
    escrow, collected, requested, expect,
    case(100u128, 200u128, 500u128, 40u128),  // 40%
    case(123u128, 0u128, 456u128, 0u128),     // no collection => no burn
    case(100u128, 1u128, 3u128, 33u128),      // floor on rounding
    case(50u128, 10u128, 0u128, 500u128)      // denom clamp to 1
)]
fn compute_burn_shares_cases(escrow: u128, collected: u128, requested: u128, expect: u128) {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let c = new_test_contract(&vault_id);

    assert_eq!(c.compute_burn_shares(escrow, collected, requested), expect);
}

#[test]
fn compute_effective_totals_fee_share_and_virtuals() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let cur = 1_500u128.into();
    let last = 1_000u128.into();
    let perf = crate::wad::Wad::one() / 10; // 10%
    let ts = 1_000u128.into();
    let vs = 1u128.into();
    let va = 1u128.into();

    let (nts, nta) = Contract::compute_effective_totals(cur, last, perf, ts, vs, va);
    let expected_fee = compute_fee_shares(cur, last, perf, ts);

    assert_eq!(nts, ts + expected_fee + vs);
    assert_eq!(nta, cur + va);
}

#[test]
fn compute_escrow_settlement_burns_min_and_refunds_rest() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let s1: (u128, u128) = Contract::compute_escrow_settlement(100, 40).into();
    assert_eq!(s1, (40u128, 60u128));

    let s2: (u128, u128) = Contract::compute_escrow_settlement(100, 200).into();
    assert_eq!(s2, (100u128, 0u128));

    let s3: (u128, u128) = Contract::compute_escrow_settlement(0, 50).into();
    assert_eq!(s3, (0u128, 0u128));
}

#[test]
fn removing_holding_market_hides_assets_and_leaves_orphan_supply() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();

    let m = mk(7001);

    // Market is known, holding > 0, with cap=0 and removal already scheduled.
    // This satisfies current preconditions in set_withdraw_queue for omission.
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(0);
    cfg.enabled = true;
    cfg.removable_at = 1; // scheduled in the past relative to the block timestamp we set below
    c.config.insert(m.clone(), cfg);
    c.market_supply.insert(m.clone(), 10);

    // Present in current withdraw queue
    c.withdraw_queue.push(m.clone());

    // Advance block timestamp so timelock precondition passes
    set_block_ts(&vault_id, &owner, 2);

    // Remove the market from the queue (new queue empty)
    c.set_withdraw_queue(vec![]);

    // Config was removed, but supply mapping still exists (orphaned)
    assert!(c.config.get(&m).is_none(), "Config should be removed");
    assert_eq!(
        *c.market_supply.get(&m).unwrap_or(&0),
        10,
        "Principal remains in market_supply but is orphaned"
    );

    // Total assets now undercount because get_total_assets sums withdraw_queue only
    assert_eq!(
        c.get_total_assets().0,
        c.idle_balance, // withdraw_queue is empty, so principal is ignored
        "Total assets should not silently drop due to queue-based accounting"
    );
}

#[test]
fn cap_zero_keeps_enabled_and_submit_removal_works() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();

    setup_env(&vault_id, &owner, vec![]);

    let m = mk(8001);

    // Seed a known, enabled market with cap > 0
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(10);
    cfg.enabled = true;
    c.config.insert(m.clone(), cfg);

    // Lower cap to zero: should NOT disable the market anymore
    c.submit_cap(m.clone(), U128(0));
    let cfg_after = c.config.get(&m).expect("market must exist");
    assert_eq!(cfg_after.cap.0, 0, "cap must be updated to 0");
    assert!(cfg_after.enabled, "enabled must remain true when cap is 0");

    set_block_ts(&vault_id, &owner, 2);

    // Now we can schedule removal
    c.submit_market_removal(m.clone());
    let cfg_after2 = c.config.get(&m).expect("market must exist");
    assert!(cfg_after2.removable_at > 0, "removal must be scheduled");
}
#[test]
fn accept_cap_raise_enables_and_cap_zero_keeps_enabled() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();

    setup_env(&vault_id, &owner, vec![]);

    let m = mk(8002);

    // Start disabled with cap=0
    c.config.insert(m.clone(), MarketConfiguration::default());

    // Submit raise -> pending
    let raise = 5u128;
    set_ctx(&vault_id, &owner, None, Some(yocto_for_new_market()));
    c.submit_cap(m.clone(), U128(raise));

    // Fast-forward timelock to accept the raise
    set_ctx(
        &vault_id,
        &owner,
        Some(env::block_timestamp() + 1_000_000_000),
        Some(yocto_for_bytes(storage_bytes_for_queue_account_id())),
    );
    c.accept_cap(m.clone());

    let cfg1 = c.config.get(&m).unwrap();
    assert_eq!(cfg1.cap.0, raise);
    assert!(cfg1.enabled, "market should be enabled after raise");
    assert!(
        c.withdraw_queue.iter().any(|x| x == &m),
        "market must be in withdraw queue after enabling"
    );

    // Now lower back to 0 (immediate path) and ensure enabled stays true
    c.submit_cap(m.clone(), U128(0));
    let cfg2 = c.config.get(&m).unwrap();
    assert_eq!(cfg2.cap.0, 0);
    assert!(cfg2.enabled, "enabled must remain true on cap=0");
}

#[test]
#[should_panic = "Policy violation: Cannot remove market with non-zero cap"]
fn set_withdraw_queue_disallow_nonzero_cap_removal() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(
        &vault_id,
        &c.own_get_owner()
            .unwrap_or_else(|| env::panic_str(&"Owner not set".to_string())),
        vec![],
    );

    let m = mk(5000);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(1); // non-zero cap
    cfg.enabled = true; // must be enabled or holding to trigger invariant
    c.config.insert(m.clone(), cfg);
    c.withdraw_queue.push(m.clone());

    // Attempt to remove from queue should panic due to non-zero cap
    c.set_withdraw_queue(vec![]);
}

#[test]
#[should_panic = "Policy violation: Cannot remove market with pending cap change"]
fn set_withdraw_queue_disallow_pending_cap_removal() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let m = mk(5001);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(0);
    cfg.enabled = true;
    c.config.insert(m.clone(), cfg);
    c.withdraw_queue.push(m.clone());

    // Insert a pending cap change
    c.pending_cap.insert(
        m.clone(),
        templar_common::vault::PendingValue {
            value: 1,
            valid_at: env::block_timestamp() + 1,
        },
    );

    // Attempt to remove from queue should panic due to pending cap change
    c.set_withdraw_queue(vec![]);
}

#[test]
#[should_panic = "Policy violation: Removal timelock not elapsed for market"]
fn set_withdraw_queue_disallow_timelock_not_elapsed() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let m = mk(5002);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(0);
    cfg.enabled = true;
    cfg.removable_at = 10; // in the future relative to block timestamp we set below
    c.config.insert(m.clone(), cfg);
    c.market_supply.insert(m.clone(), 1); // non-zero supply enforces timelock path
    c.withdraw_queue.push(m.clone());

    // Set block timestamp below removable_at so timelock has not elapsed
    set_block_ts(&vault_id, &owner, 5);

    // Attempt to remove from queue should panic due to timelock not elapsed
    c.set_withdraw_queue(vec![]);
}

#[test]
fn set_withdraw_queue_allows_zero_supply_removal() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(
        &vault_id,
        &c.own_get_owner()
            .unwrap_or_else(|| env::panic_str(&"Owner not set".to_string())),
        vec![],
    );

    let m = mk(5003);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(0);
    cfg.enabled = true;
    // removable_at irrelevant when supply is zero
    c.config.insert(m.clone(), cfg);
    c.withdraw_queue.push(m.clone());

    // Supply is zero; removal should be allowed immediately
    c.set_withdraw_queue(vec![]);

    // Config should be deleted
    assert!(
        c.config.get(&m).is_none(),
        "Config must be removed for omitted market with zero supply"
    );
    // And the queue should be empty
    assert!(
        !c.withdraw_queue.iter().any(|x| x == &m),
        "Withdraw queue must not contain the removed market"
    );
}

#[test]
#[should_panic = "Policy violation: Unknown market in new queue"]
fn set_withdraw_queue_rejects_unknown_market() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &c.own_get_owner().unwrap(), vec![]);

    // No config for this market
    let unknown = mk(5999);
    c.set_withdraw_queue(vec![unknown]);
}

#[rstest(
    before,
    new_principal,
    need,
    rem,
    coll,
    case(100u128, 55u128, 40u128, 50u128, 10u128),
    case(100u128, 80u128, 40u128, 50u128, 10u128),
    case(0u128, 0u128, 0u128, 0u128, 0u128),
    case(1000u128, 1000u128, 500u128, 800u128, 100u128),
    case(200u128, 0u128, 300u128, 0u128, 0u128)
)]
fn reconcile_withdraw_outcome_invariants_cases(
    before: u128,
    new_principal: u128,
    need: u128,
    rem: u128,
    coll: u128,
) {
    let WithdrawReconciliation {
        payout_delta,
        remaining_next,
        collected_next,
        idle_delta,
    } = crate::impl_callbacks::reconcile_withdraw_outcome(before, new_principal, rem, coll);

    let withdrawn = before.saturating_sub(new_principal);
    let expected_credited = withdrawn.min(need);

    assert_eq!(payout_delta, expected_credited);
    assert!(payout_delta <= need);
    assert_eq!(remaining_next, rem.saturating_sub(payout_delta));
    assert_eq!(collected_next, coll.saturating_add(payout_delta));
    assert_eq!(idle_delta, payout_delta);
}

#[rstest(
    assets,
    shares,
    case(0u128, 0u128),
    case(1u128, 1u128),
    case(1_000_000_000_000_000_000u128, 1u128),
    case(123_456_789u128, 987_654_321u128),
    case(1u128, 1_000_000_000_000_000_000u128)
)]
fn convert_roundtrip_bounds_cases(assets: u128, shares: u128) {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let c = new_test_contract(&vault_id);

    let to_sh = c.convert_to_shares(U128(assets));
    let back_a = c.convert_to_assets(to_sh);
    assert!(
        back_a.0 <= assets,
        "assets->shares->assets must not increase"
    );

    let to_a = c.convert_to_assets(U128(shares));
    let back_s = c.convert_to_shares(to_a);
    assert!(
        back_s.0 >= shares,
        "shares->assets->shares must not decrease"
    );
}

#[rstest(
    cap,
    cur,
    idle,
    req,
    case(100u128, 60u128, 80u128, None),
    case(100u128, 0u128, 80u128, Some(50u128)),
    case(10u128, 10u128, 80u128, None),
    case(0u128, 0u128, 0u128, Some(1u128))
)]
fn clamp_allocation_total_matches_min_bounds_cases(
    cap: u128,
    cur: u128,
    idle: u128,
    req: Option<u128>,
) {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let m = mk(1);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(cap);
    cfg.enabled = cap > 0;
    c.config.insert(m.clone(), cfg);
    c.market_supply.insert(m.clone(), cur);
    c.supply_queue.push(m.clone());
    c.idle_balance = idle;

    let room = cap.saturating_sub(cur);
    let requested = req.unwrap_or(c.idle_balance);
    let expect = requested.min(c.idle_balance).min(room);

    let got = c.clamp_allocation_total(req);
    assert_eq!(got, expect);
}

#[rstest(
    principal,
    idle,
    case(0u128, 0u128),
    case(123u128, 0u128),
    case(0u128, 456u128),
    case(789u128, 1_011u128)
)]
fn total_assets_ignores_offqueue_cases(principal: u128, idle: u128) {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let mut c = new_test_contract(&vault_id);

    let m = mk(7003);
    c.config.insert(m.clone(), MarketConfiguration::default());
    c.market_supply.insert(m.clone(), principal);
    c.idle_balance = idle;

    assert_eq!(c.get_total_assets().0, idle);
}

#[test]
fn set_fee_recipient_accrues_before_switch() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = accounts(1);
    setup_env(&vault_id, &owner, vec![]);

    // Seed supply so fee shares can mint
    c.deposit_unchecked(&accounts(1), 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    // Simulate profit: last=1000, current=1500
    c.idle_balance = 1_500;
    c.last_total_assets = 1_000;
    c.performance_fee = crate::wad::Wad::one() / 10;

    let cur = c.get_total_assets().0;
    let ts_before = c.total_supply();
    let expect = compute_fee_shares(
        cur.into(),
        1_000.into(),
        c.performance_fee,
        ts_before.into(),
    );

    let old_recipient = c.fee_recipient.clone();
    let old_balance = c.balance_of(&old_recipient);

    // Switch fee recipient; should accrue to old recipient first
    let new_recipient = accounts(3);
    c.set_fee_recipient(new_recipient.clone());

    assert_eq!(
        c.balance_of(&old_recipient),
        old_balance + expect.as_u128_trunc(),
        "fees must accrue to the old recipient before switching"
    );
    assert_eq!(
        c.total_supply(),
        ts_before + expect.as_u128_trunc(),
        "total supply must increase by minted fee shares"
    );
    assert_eq!(
        c.fee_recipient, new_recipient,
        "recipient should be updated"
    );
    assert_eq!(
        c.last_total_assets, cur,
        "last_total_assets must update to current after accrual"
    );
}

#[test]
fn set_fee_recipient_accrues_before_switch_variant() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = accounts(1);
    setup_env(&vault_id, &owner, vec![]);

    // Seed supply so fee shares can mint
    c.deposit_unchecked(&accounts(2), 2_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    // Simulate profit: last=2000, current=2400
    c.idle_balance = 2_400;
    c.last_total_assets = 2_000;
    c.performance_fee = crate::wad::Wad::one() / 20; // 5%

    let cur = c.get_total_assets().0;
    let ts_before = c.total_supply();
    let expect = compute_fee_shares(
        cur.into(),
        2_000.into(),
        c.performance_fee,
        ts_before.into(),
    );

    let old_recipient = c.fee_recipient.clone();
    let old_balance = c.balance_of(&old_recipient);

    // Switch fee recipient; should accrue to old recipient first
    let new_recipient = accounts(3);
    c.set_fee_recipient(new_recipient.clone());

    assert_eq!(
        c.balance_of(&old_recipient),
        old_balance + expect.as_u128_trunc(),
        "fees must accrue to the old recipient before switching"
    );
    assert_eq!(
        c.total_supply(),
        ts_before + expect.as_u128_trunc(),
        "total supply must increase by minted fee shares"
    );
    assert_eq!(
        c.fee_recipient, new_recipient,
        "recipient should be updated"
    );
    assert_eq!(
        c.last_total_assets, cur,
        "last_total_assets must update to current after accrual"
    );
}

#[test]
fn set_performance_fee_accrues_with_old_rate_then_updates() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c
        .own_get_owner()
        .unwrap_or_else(|| env::panic_str(&"Owner not set".to_string()));
    setup_env(&vault_id, &owner, vec![]);

    // Seed supply so fee shares can mint
    c.deposit_unchecked(&accounts(1), 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    // Simulate profit: last=1000, current=1500
    c.idle_balance = 1_500;
    c.last_total_assets = 1_000;

    // Old rate = 10%, new rate = 1%
    c.performance_fee = crate::wad::Wad::one() / 10;
    let cur = c.get_total_assets().0;
    let ts_before = c.total_supply();
    let expect_old = compute_fee_shares(
        cur.into(),
        1_000.into(),
        c.performance_fee,
        ts_before.into(),
    );

    let recipient = c.fee_recipient.clone();
    let bal_before = c.balance_of(&recipient);

    c.set_performance_fee(U128(u128::from(crate::wad::Wad::one() / 100)));

    assert_eq!(
        c.balance_of(&recipient),
        bal_before + expect_old.as_u128_trunc(),
        "accrual must use the old fee rate before updating"
    );
    assert_eq!(
        c.total_supply(),
        ts_before + expect_old.as_u128_trunc(),
        "total supply must reflect fee shares minted at old rate"
    );
    assert_eq!(
        c.performance_fee,
        crate::wad::Wad::one() / 100,
        "performance fee must be updated to the new rate"
    );
    assert_eq!(
        c.last_total_assets, cur,
        "last_total_assets must update to current after accrual"
    );
}

#[test]
fn set_performance_fee_accrues_with_old_rate_then_updates_variant() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c
        .own_get_owner()
        .unwrap_or_else(|| env::panic_str(&"Owner not set".to_string()));
    setup_env(&vault_id, &owner, vec![]);

    // Seed supply so fee shares can mint
    c.deposit_unchecked(&accounts(2), 2_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    // Simulate profit: last=2000, current=2400
    c.idle_balance = 2_400;
    c.last_total_assets = 2_000;

    // Old rate = 5%, new rate = 0.5%
    c.performance_fee = crate::wad::Wad::one() / 20; // 5%
    let cur = c.get_total_assets().0;
    let ts_before = c.total_supply();
    let expect_old = compute_fee_shares(
        cur.into(),
        2_000.into(),
        c.performance_fee,
        ts_before.into(),
    );

    let recipient = c.fee_recipient.clone();
    let bal_before = c.balance_of(&recipient);

    c.set_performance_fee(U128(u128::from(crate::wad::Wad::one() / 200))); // 0.5%

    assert_eq!(
        c.balance_of(&recipient),
        bal_before + expect_old.as_u128_trunc(),
        "accrual must use the old fee rate before updating"
    );
    assert_eq!(
        c.total_supply(),
        ts_before + expect_old.as_u128_trunc(),
        "total supply must reflect fee shares minted at old rate"
    );
    assert_eq!(
        c.performance_fee,
        crate::wad::Wad::one() / 200,
        "performance fee must be updated to the new rate"
    );
    assert_eq!(
        c.last_total_assets, cur,
        "last_total_assets must update to current after accrual"
    );
}

#[test]
fn internal_accrue_fee_mints_zero_on_loss_and_updates_last() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Seed supply so total_supply > 0
    c.deposit_unchecked(&accounts(1), 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    // Loss scenario: last=1000, current=800
    c.idle_balance = 800;
    c.last_total_assets = 1_000;
    c.performance_fee = crate::wad::Wad::one() / 10;

    let ts_before = c.total_supply();
    let fr = c.fee_recipient.clone();
    let bal_before = c.balance_of(&fr);
    let cur = c.get_total_assets().0;

    c.internal_accrue_fee();

    assert_eq!(
        c.total_supply(),
        ts_before,
        "no shares should be minted when cur < last_total_assets"
    );
    assert_eq!(
        c.balance_of(&fr),
        bal_before,
        "fee recipient balance must remain unchanged on loss"
    );
    assert_eq!(
        c.last_total_assets, cur,
        "last_total_assets must update to current even on loss"
    );
}

#[rstest]
fn ft_on_transfer_supply_accepts_full_and_mints_shares(
    mut c_asset_env: Contract,
    enabled_market_100: (AccountId, MarketConfiguration),
) {
    let mut c = c_asset_env;
    c.mode = AllocationMode::Eager {
        min_batch: U128(u128::MAX),
    };
    let (m, cfg) = enabled_market_100;
    c.config.insert(m.clone(), cfg);
    c.supply_queue.push(m);

    let sender = accounts(1);
    let deposit = 50u128;
    let expect_shares = c.preview_deposit(U128(deposit)).0;

    let res = c.ft_on_transfer(
        sender.clone(),
        U128(deposit),
        serde_json::to_string(&DepositMsg::Supply).unwrap(),
    );
    match res {
        PromiseOrValue::Value(U128(refund)) => assert_eq!(refund, 0, "no refund expected"),
        _ => panic!("expected Value refund"),
    }

    assert_eq!(
        c.balance_of(&sender),
        expect_shares,
        "sender must receive expected shares"
    );
    assert_eq!(
        c.idle_balance, deposit,
        "idle must increase by accepted deposit"
    );
    assert_eq!(
        c.last_total_assets, deposit,
        "last_total_assets must increase by accepted deposit"
    );
    assert!(
        matches!(c.op_state, OpState::Idle),
        "must remain idle when min_batch not reached"
    );
}

#[rstest]
fn ft_on_transfer_supply_partial_refund_when_capped(
    mut c_asset_env: Contract,
    enabled_market_100: (AccountId, MarketConfiguration),
) {
    let mut c = c_asset_env;
    c.mode = AllocationMode::Eager {
        min_batch: U128(u128::MAX),
    };
    let (m, mut cfg) = enabled_market_100;
    cfg.cap = U128(50); // override cap for this case
    c.config.insert(m.clone(), cfg);
    c.supply_queue.push(m);

    let sender = accounts(2);
    let deposit = 80u128;
    let accept = 50u128;
    let expect_shares = c.preview_deposit(U128(accept)).0;

    let res = c.ft_on_transfer(
        sender.clone(),
        U128(deposit),
        serde_json::to_string(&DepositMsg::Supply).unwrap(),
    );
    match res {
        PromiseOrValue::Value(U128(refund)) => assert_eq!(refund, deposit - accept),
        _ => panic!("expected Value refund"),
    }

    assert_eq!(
        c.balance_of(&sender),
        expect_shares,
        "shares minted must equal accepted amount preview"
    );
    assert_eq!(
        c.idle_balance, accept,
        "idle increases by accepted amount only"
    );
    assert_eq!(
        c.last_total_assets, accept,
        "last_total_assets increases by accepted amount only"
    );
}

#[test]
fn ft_on_transfer_wrong_token_full_refund_via_receiver() {
    // Underlying token id != predecessor => full refund
    let vault_id = accounts(0);
    let mut c = new_test_contract(&mk(42)); // underlying differs from predecessor
    setup_env(&vault_id, &vault_id, vec![]);

    c.mode = AllocationMode::Eager {
        min_batch: U128(u128::MAX),
    };

    // Provide a market (not used due to wrong token)
    let m = mk(9003);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(100);
    cfg.enabled = true;
    c.config.insert(m.clone(), cfg);
    c.supply_queue.push(m);

    let sender = accounts(3);
    let deposit = 70u128;

    let res = c.ft_on_transfer(
        sender.clone(),
        U128(deposit),
        serde_json::to_string(&DepositMsg::Supply).unwrap(),
    );
    match res {
        PromiseOrValue::Value(U128(refund)) => assert_eq!(refund, deposit, "full refund expected"),
        _ => panic!("expected Value refund"),
    }
    assert_eq!(c.balance_of(&sender), 0, "no shares should be minted");
    assert_eq!(c.idle_balance, 0, "idle must remain unchanged");
}

#[test]
#[should_panic = "Invalid deposit msg"]
fn ft_on_transfer_invalid_msg_panics() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &vault_id, vec![]);

    let _ = c.ft_on_transfer(accounts(4), U128(10), "not-json".into());
}

#[rstest]
fn ft_on_transfer_zero_amount_returns_zero_refund(
    mut c_vault_env: Contract,
    enabled_market_100: (AccountId, MarketConfiguration),
) {
    let mut c = c_vault_env;

    // Setup a valid market
    let (m, cfg) = enabled_market_100;
    c.config.insert(m.clone(), cfg);
    c.supply_queue.push(m);

    let sender = accounts(5);
    let bal_before = c.balance_of(&sender);

    let res = c.ft_on_transfer(
        sender.clone(),
        U128(0),
        serde_json::to_string(&DepositMsg::Supply).unwrap(),
    );
    match res {
        PromiseOrValue::Value(U128(refund)) => assert_eq!(refund, 0),
        _ => panic!("expected Value refund"),
    }
    assert_eq!(
        c.balance_of(&sender),
        bal_before,
        "no shares should be minted"
    );
}

#[rstest]
fn ft_on_transfer_eager_mode_triggers_allocation(
    mut c_asset_env: Contract,
    enabled_market_100: (AccountId, MarketConfiguration),
) {
    let mut c = c_asset_env;

    // Trigger eager allocation with any positive deposit
    c.mode = AllocationMode::Eager { min_batch: U128(1) };

    // Valid market/cap
    let (m, cfg) = enabled_market_100;
    c.config.insert(m.clone(), cfg);
    c.supply_queue.push(m);

    let deposit = 5u128;

    let res = c.ft_on_transfer(
        c.underlying_asset.contract_id().into(),
        U128(deposit),
        serde_json::to_string(&DepositMsg::Supply).unwrap(),
    );

    match res {
        PromiseOrValue::Value(U128(refund)) => assert_eq!(refund, 0),
        _ => panic!("expected Value refund"),
    }

    assert!(
        matches!(c.op_state, OpState::Allocating { .. }),
        "Eager mode must trigger allocation"
    );
    assert_eq!(
        c.idle_balance, 0,
        "idle should be reserved by start_allocation"
    );
}

#[test]
#[should_panic = "Invalid deposit msg"]
fn mt_on_transfer_invalid_msg_panics() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &vault_id, vec![]);

    let _ = c.mt_on_transfer(
        accounts(1),
        vec![accounts(1)],
        vec!["t".to_string()],
        vec![U128(1)],
        "bad".into(),
    );
}

#[test]
#[should_panic = "This contract only accepts one token at a time."]
fn mt_on_transfer_rejects_multiple_tokens() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &vault_id, vec![]);

    let _ = c.mt_on_transfer(
        accounts(2),
        vec![accounts(2)],
        vec!["a".to_string(), "b".to_string()], // len != 1
        vec![U128(1)],
        serde_json::to_string(&DepositMsg::Supply).unwrap(),
    );
}

#[test]
#[should_panic = "Invalid input length"]
fn mt_on_transfer_rejects_invalid_input_lengths() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &vault_id, vec![]);

    let _ = c.mt_on_transfer(
        accounts(3),
        vec![accounts(3), accounts(4)], // len != 1
        vec!["t".to_string()],
        vec![U128(1)],
        serde_json::to_string(&DepositMsg::Supply).unwrap(),
    );
}

#[test]
fn mt_on_transfer_wrong_asset_refunds_full() {
    // With default test underlying (NEP-141), is_nep245 should fail; expect full refund
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &vault_id, vec![]);

    let sender = accounts(5);
    let amount = 25u128;

    let res = c.mt_on_transfer(
        accounts(3),                 // sender_id (ignored in logic)
        vec![sender.clone()],        // previous_owner_ids
        vec!["token-1".to_string()], // token_ids
        vec![U128(amount)],          // amounts
        serde_json::to_string(&DepositMsg::Supply).unwrap(),
    );
    match res {
        PromiseOrValue::Value(refunds) => {
            assert_eq!(refunds.len(), 1);
            assert_eq!(refunds[0].0, amount, "full refund expected for wrong asset");
        }
        _ => panic!("expected Value refund"),
    }
    assert_eq!(c.balance_of(&sender), 0, "no shares should be minted");
    assert_eq!(c.idle_balance, 0, "idle must remain unchanged");
}

#[test]
fn execute_supply_zero_amount_rejected() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &vault_id, vec![]);

    let sender = accounts(4);
    let refund = c.execute_supply(sender.clone(), vault_id.clone(), 0);
    assert_eq!(refund, 0, "zero deposit returns zero refund");
    assert_eq!(c.balance_of(&sender), 0, "no shares should be minted");
}

#[test]
fn governance_set_curator_grants_allocator() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    // Prepare a market to exercise allocator permission
    let m1 = mk(9101);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(1);
    cfg.enabled = true;
    c.config.insert(m1.clone(), cfg);

    let new_cur = accounts(3);
    c.set_curator(new_cur.clone());

    // New curator can set supply queue
    set_ctx(
        &vault_id,
        &new_cur,
        None,
        Some(yocto_for_bytes(storage_bytes_for_queue_account_id())),
    );
    c.set_supply_queue(vec![m1.clone()]);
    assert_eq!(c.supply_queue.len(), 1);
    assert_eq!(c.supply_queue.get(0), Some(&m1));
}

#[test]
fn governance_set_is_allocator_grant_allows_queue_ops() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let grantee = accounts(4);

    // Market to operate on
    let m1 = mk(9102);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(1);
    cfg.enabled = true;
    c.config.insert(m1.clone(), cfg);

    // Grant Allocator role
    c.set_is_allocator(grantee.clone(), true);

    // Grantee can set supply queue
    set_ctx(
        &vault_id,
        &grantee,
        None,
        Some(yocto_for_bytes(storage_bytes_for_queue_account_id())),
    );
    c.set_supply_queue(vec![m1.clone()]);
    assert_eq!(c.supply_queue.len(), 1);
    assert_eq!(c.supply_queue.get(0), Some(&m1));
}

#[test]
#[should_panic]
fn governance_set_is_allocator_revoke_disallows_queue_ops() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let grantee = accounts(12);
    c.set_is_allocator(grantee.clone(), true);

    // Market to attempt on
    let m1 = mk(9103);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(1);
    cfg.enabled = true;
    c.config.insert(m1.clone(), cfg);

    // Revoke Allocator role; subsequent queue op by grantee should panic due to lack of rights
    c.set_is_allocator(grantee.clone(), false);
    set_ctx(
        &vault_id,
        &grantee,
        None,
        Some(yocto_for_bytes(storage_bytes_for_queue_account_id())),
    );
    c.set_supply_queue(vec![m1]);
}

#[test]
#[should_panic = "not yet"]
fn governance_accept_guardian_not_yet_panics() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    c.timelock_ns = u64::MAX;

    let new_g = accounts(5);
    c.submit_guardian(new_g);
    // Timelock not advanced -> should panic
    c.accept_guardian();
}

#[test]
fn governance_submit_accept_and_revoke_guardian() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let new_g = accounts(4);
    c.submit_guardian(new_g.clone());

    // Advance time beyond timelock and accept
    set_ctx(
        &vault_id,
        &owner,
        Some(env::block_timestamp() + 1_000_000_000),
        None,
    );
    c.accept_guardian();

    // Stage another pending and then revoke it
    let another = accounts(3);
    set_ctx(&vault_id, &owner, None, None);
    c.submit_guardian(another);
    c.revoke_pending_guardian();

    // No pending now; accept should no-op (but must not panic)
    c.accept_guardian();
}

#[test]
fn governance_submit_accept_timelock_increase_then_decrease() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let cur = c.get_configuration().initial_timelock_ns;

    // Increase applies immediately
    c.submit_timelock((cur.0 + 1).into());
    assert_eq!(
        c.get_configuration().initial_timelock_ns.0,
        cur.0 + 1,
        "timelock should increase immediately"
    );

    // Decrease schedules a pending change
    c.submit_timelock(cur);
    set_ctx(
        &vault_id,
        &owner,
        Some(env::block_timestamp() + 1_000_000_000),
        None,
    );
    c.accept_timelock();
    assert_eq!(
        c.get_configuration().initial_timelock_ns,
        cur,
        "timelock should decrease after accept"
    );
}

#[test]
#[should_panic = "No pending timelock change"]
fn governance_accept_timelock_without_pending_panics() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    // No pending change -> accept should panic
    c.accept_timelock();
}

#[test]
#[should_panic = "No pending timelock change"]
fn governance_revoke_pending_timelock_then_accept_panics() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let cur = c.get_configuration().initial_timelock_ns;

    // Force a pending by first increasing then decreasing
    c.submit_timelock((cur.0 + 1).into());
    c.submit_timelock(cur);

    // Revoke the pending change; accept must now panic
    c.revoke_pending_timelock();
    c.accept_timelock();
}

#[test]
fn governance_submit_cap_immediate_decrease() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let m = mk(9104);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(10);
    cfg.enabled = true;
    c.config.insert(m.clone(), cfg);

    c.submit_cap(m.clone(), U128(3));
    let after = c.config.get(&m).unwrap();
    assert_eq!(after.cap, U128(3));
}

#[test]
fn governance_submit_and_accept_cap_new_market_creates_and_enables() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let m = mk(9105);

    // Submit raise for a brand-new market
    set_ctx(
        &vault_id,
        &owner,
        None,
        Some(yocto_for_new_market() + yocto_for_pending_cap()),
    );
    c.submit_cap(m.clone(), U128(5));

    // Advance timelock and accept; attach storage for withdraw queue addition
    set_ctx(
        &vault_id,
        &owner,
        Some(env::block_timestamp() + 1_000_000_000),
        Some(yocto_for_bytes(storage_bytes_for_queue_account_id())),
    );
    c.accept_cap(m.clone());

    let cfg = c.config.get(&m).unwrap();
    assert_eq!(cfg.cap.0, 5);
    assert!(
        cfg.enabled,
        "market should be enabled after accepting raise"
    );
    assert!(
        c.withdraw_queue.iter().any(|x| x == &m),
        "market must be in withdraw queue after enabling"
    );
}

#[test]
#[should_panic = "No pending cap change for this market"]
fn governance_revoke_pending_cap_then_accept_panics() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let m = mk(9106);

    // Create pending cap raise for a new market
    set_ctx(
        &vault_id,
        &owner,
        None,
        Some(yocto_for_new_market() + yocto_for_pending_cap()),
    );
    c.submit_cap(m.clone(), U128(7));

    // Revoke, then accepting should panic
    set_ctx(&vault_id, &owner, None, None);
    c.revoke_pending_cap(m.clone());
    c.accept_cap(m);
}

#[test]
fn governance_submit_and_revoke_market_removal() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    c.timelock_ns = 1;
    let m = mk(9107);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = U128(0);
    cfg.enabled = true;
    c.config.insert(m.clone(), cfg);

    // Submit removal (schedules timelock)
    c.submit_market_removal(m.clone());
    let after = c.config.get(&m).unwrap();
    assert!(after.removable_at > 0, "removal must be scheduled");

    // Revoke pending removal
    c.revoke_pending_market_removal(m.clone());
    let after2 = c.config.get(&m).unwrap();
    assert_eq!(after2.removable_at, 0, "removal must be revoked");
}

#[test]
fn governance_set_skim_recipient_updates_field() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = accounts(1);
    setup_env(&vault_id, &owner, vec![]);

    let new_recipient = accounts(4);
    c.set_skim_recipient(new_recipient.clone());
    assert_eq!(c.skim_recipient, new_recipient);
}

#[test]
fn governance_set_fee_recipient_no_fee_does_not_accrue() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = accounts(1);
    setup_env(&vault_id, &owner, vec![]);

    // Seed supply and simulate profit, but fee = 0
    c.deposit_unchecked(&owner, 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    c.idle_balance = 1_500;
    c.last_total_assets = 1_000;
    c.performance_fee = crate::wad::Wad::zero();

    let ts_before = c.total_supply();
    let last_before = c.last_total_assets;

    let new_recipient = accounts(5);
    c.set_fee_recipient(new_recipient.clone());

    assert_eq!(
        c.total_supply(),
        ts_before,
        "no fee shares minted when fee=0"
    );
    assert_eq!(
        c.last_total_assets, last_before,
        "last_total_assets should not change when fee=0"
    );
    assert_eq!(c.fee_recipient, new_recipient);
}

#[test]
fn governance_set_withdraw_queue_happy_path() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    // Two enabled markets
    let m1 = mk(9201);
    let m2 = mk(9202);
    for m in [&m1, &m2] {
        let mut cfg = MarketConfiguration::default();
        cfg.cap = U128(1);
        cfg.enabled = true;
        c.config.insert(m.clone(), cfg);
    }

    set_ctx(
        &vault_id,
        &owner,
        None,
        Some(2 * yocto_for_bytes(storage_bytes_for_queue_account_id())),
    );
    c.set_withdraw_queue(vec![m1.clone(), m2.clone()]);

    assert_eq!(c.withdraw_queue.len(), 2);
    assert_eq!(c.withdraw_queue.get(0), Some(&m1));
    assert_eq!(c.withdraw_queue.get(1), Some(&m2));
}

#[test]
fn test_prevent_skim_underlying_and_shares() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    // Set a skim recipient
    let recipient = accounts(8);
    c.set_skim_recipient(recipient.clone());

    // Seed idle underlying and escrow some shares (held by the vault itself)
    c.idle_balance = 123;
    c.deposit_unchecked(&vault_id, 100)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));

    // Snapshot pre-state
    let pre_idle = c.idle_balance;
    let pre_vault_shares = c.balance_of(&vault_id);
    let pre_recipient_shares = c.balance_of(&recipient);

    // Attempt to skim underlying token -> must panic
    let underlying: AccountId = c.underlying_asset.contract_id().into();
    let r1 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = c.skim(underlying.clone());
    }));
    assert!(r1.is_err(), "skimming underlying token should panic");

    // Attempt to skim the share token -> must panic
    let share_token: AccountId = vault_id.clone();
    let r2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = c.skim(share_token.clone());
    }));
    assert!(r2.is_err(), "skimming share token should panic");

    // State must be unchanged
    assert_eq!(c.idle_balance, pre_idle, "idle balance must be unchanged");
    assert_eq!(
        c.balance_of(&vault_id),
        pre_vault_shares,
        "vault's escrowed shares must be unchanged"
    );
    assert_eq!(
        c.balance_of(&recipient),
        pre_recipient_shares,
        "skim recipient must not receive any shares"
    );
    assert!(
        matches!(c.op_state, OpState::Idle),
        "op_state must remain Idle"
    );
}

#[rstest]
fn after_supply_1_check_allocating_not_allocating(mut c_max: Contract) {
    let mut c = c_max;

    c.op_state = OpState::Idle;

    c.after_supply_1_check(Ok(U128(1)), 0, 2, Default::default());

    assert_eq!(c.op_state, OpState::Idle);
    assert_eq!(c.plan, None);
}

#[test]
fn after_supply_1_check_allocating_not_allocating_index() {
    let vault_id = accounts(0);
    setup_env(
        &vault_id,
        &vault_id,
        vec![PromiseResult::Successful(
            near_sdk::serde_json::to_vec(&U128(u128::MAX))
                .unwrap_or_else(|e| near_sdk::env::panic_str(&e.to_string())),
        )],
    );

    let mut c = new_test_contract(&vault_id);

    let op_id = 1;
    let receiver = mk(7);

    c.op_state = OpState::Allocating {
        op_id,
        index: 0u32,
        remaining: 0u128,
    };

    c.after_supply_1_check(Ok(U128(1)), op_id + 1, 0, Default::default());

    assert_eq!(c.op_state, OpState::Idle);
    assert_eq!(c.plan, None);
}

#[test]
fn after_supply_1_check_allocating() {
    let vault_id = accounts(0);
    setup_env(
        &vault_id,
        &vault_id,
        vec![PromiseResult::Successful(
            near_sdk::serde_json::to_vec(&U128(u128::MAX))
                .unwrap_or_else(|e| near_sdk::env::panic_str(&e.to_string())),
        )],
    );

    let mut c = new_test_contract(&vault_id);

    let op_id = 1;
    let receiver = mk(7);

    c.op_state = OpState::Allocating {
        op_id,
        index: 0u32,
        remaining: 0u128,
    };

    c.after_supply_1_check(Ok(U128(1)), op_id, 0, Default::default());

    assert_eq!(c.op_state, OpState::Idle);
    assert_eq!(c.plan, None);
}

#[test]
fn after_send_to_user_success_no_escrow() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let mut c = new_test_contract(&vault_id);

    let receiver = mk(7);

    c.idle_balance = 1_000;
    c.op_state = OpState::Payout {
        op_id: 1,
        receiver: receiver.clone(),
        amount: 200,
        owner: accounts(1),
        escrow_shares: 0,
        burn_shares: 0,
    };

    let ok = c.after_send_to_user(Ok(()), 1, receiver.clone(), U128(200));
    assert!(ok, "Payout should report success");
    assert_eq!(c.idle_balance, 800, "Idle balance must decrease by payout");
    assert!(
        matches!(c.op_state, OpState::Idle),
        "Vault must go Idle after successful payout"
    );
}

#[rstest]
fn after_exec_withdraw_read_none_to_payout(mut c: Contract) {
    // Prepare a single-market withdraw queue with non-zero principal
    let market = mk(8);
    c.withdraw_queue.push(market.clone());
    c.market_supply.insert(market.clone(), 100);

    // Withdrawing: need 60, already collected 10; expect position None => new_principal = 0, withdrawn = 100, credited = min(100, 60) = 60
    c.op_state = OpState::Withdrawing {
        op_id: 42,
        index: 0,
        remaining: 60,
        receiver: mk(9),
        collected: 10,
        owner: accounts(1),
        escrow_shares: 50,
    };

    let res = c.after_exec_withdraw_read(Ok(None), 42, 0, U128(100), U128(60));

    match res {
        PromiseOrValue::Promise(p) => {}
        _ => panic!("Expected a Promise to send payout"),
    }

    assert_eq!(
        *c.market_supply.get(&market).unwrap_or(&u128::MAX),
        0,
        "Market principal should be updated to 0"
    );

    assert_eq!(
        c.idle_balance, 100,
        "Idle balance should increase by returned amount"
    );

    // State should transition to Payout with amount = collected (10) + credited (60) = 70
    match &c.op_state {
        OpState::Payout { amount, .. } => {
            assert_eq!(*amount, 70, "Payout amount must match collected + credited");
        }
        other => panic!("Unexpected state after read: {other:?}"),
    }
}

#[test]
fn after_skim_balance_zero_noop() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let mut c = new_test_contract(&vault_id);

    let res = c.after_skim_balance(Ok(U128(0)), mk(10), mk(11));
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Skim with zero balance must be a no-op"),
    }
}

#[test]
fn after_skim_balance_positive_returns_promise() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let mut c = new_test_contract(&vault_id);

    // Positive balance -> Promise to ft_transfer
    let res = c.after_skim_balance(Ok(U128(123)), mk(10), mk(11));
    match res {
        PromiseOrValue::Promise(_) => { //NOTE: one day we will be able to read the promise
             //definition :<
        }
        _ => panic!("Skim with positive balance must return a Promise"),
    }
}

/// Property: Payout failure keeps idle_balance unchanged and does not burn escrow
#[rstest(
    idle => [0u128, 1, 100],
    escrow => [0u128, 1, 50],
    amount => [0u128, 1, 25]
)]
fn prop_after_send_to_user_failure_keeps_idle(idle: u128, escrow: u128, amount: u128) {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let receiver = mk(7);
    let owner = accounts(1);

    if escrow > 0 {
        use near_sdk_contract_tools::ft::Nep141Controller as _;

        c.deposit_unchecked(&near_sdk::env::current_account_id(), escrow)
            .unwrap_or_else(|e| near_sdk::env::panic_str(&e.to_string()));
    }

    c.idle_balance = idle;
    c.op_state = OpState::Payout {
        op_id: 1,
        receiver: receiver.clone(),
        amount,
        owner: owner.clone(),
        escrow_shares: escrow,
        burn_shares: escrow,
    };

    let before = c.idle_balance;
    let ok = c.after_send_to_user(
        Err(near_sdk::PromiseError::Failed),
        1,
        receiver.clone(),
        U128(amount),
    );
    assert!(!ok, "Payout failure should return false");
    assert_eq!(
        c.idle_balance, before,
        "idle_balance must stay the same on payout failure"
    );
    assert!(
        matches!(c.op_state, OpState::Idle),
        "Vault must go Idle after payout failure"
    );
}

/// Property: Create-withdraw failure skips to next market and if collected>0 ends in Payout
#[rstest(
    collected => [1u128, 10u128],
    need => [1u128, 5u128]
)]
fn prop_after_create_withdraw_req_failure_skips(collected: u128, need: u128) {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Single-market queue so advancing index reaches end-of-queue
    let market = mk(8);
    c.withdraw_queue.push(market.clone());
    c.market_supply.insert(market.clone(), 100);

    c.op_state = OpState::Withdrawing {
        op_id: 7,
        index: 0,
        remaining: need,
        receiver: mk(9),
        collected,
        owner: accounts(1),
        escrow_shares: 0,
    };

    let res = c.after_create_withdraw_req(Err(near_sdk::PromiseError::Failed), 7, 0, U128(need));
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise after skipping to payout at end-of-queue"),
    }

    match &c.op_state {
        OpState::Payout { amount, .. } => {
            assert_eq!(*amount, collected, "Payout amount must equal collected");
        }
        other => panic!("Unexpected state: {other:?}"),
    }
}

/// Property: Exec-withdraw read failure assumes unchanged principal and does not credit idle
#[rstest(
    before => [0u128, 1u128, 100u128],
    need => [0u128, 1u128, 50u128],
    collected => [1u128, 2u128]
)]
fn prop_after_exec_withdraw_read_err_no_change(before: u128, need: u128, collected: u128) {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let market = mk(8);
    c.withdraw_queue.push(market.clone());
    c.market_supply.insert(market.clone(), before);

    let initial_idle = c.idle_balance;

    c.op_state = OpState::Withdrawing {
        op_id: 99,
        index: 0,
        remaining: need,
        receiver: mk(9),
        collected,
        owner: accounts(1),
        escrow_shares: 0,
    };

    let res = c.after_exec_withdraw_read(
        Err(near_sdk::PromiseError::Failed),
        99,
        0,
        U128(before),
        U128(need),
    );
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise to send payout at end-of-queue"),
    }

    assert_eq!(
        *c.market_supply.get(&market).unwrap_or(&u128::MAX),
        before,
        "principal must remain unchanged on read failure"
    );
    assert_eq!(
        c.idle_balance, initial_idle,
        "idle_balance must not change when nothing credited"
    );

    match &c.op_state {
        OpState::Payout { amount, .. } => {
            assert_eq!(*amount, collected, "Payout amount must equal collected");
        }
        other => panic!("Unexpected state: {other:?}"),
    }
}

/// Property: Callbacks must match current op_id or index; otherwise stop and go Idle
#[rstest(
    pass_op => [false, true],
    pass_index => [false, true]
)]
fn prop_after_exec_withdraw_read_requires_current_state(pass_op: bool, pass_index: bool) {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let market = mk(8);
    c.withdraw_queue.push(market.clone());
    c.market_supply.insert(market.clone(), 10);

    let real_op = 5u64;
    let real_idx = 0u32;

    c.op_state = OpState::Withdrawing {
        op_id: real_op,
        index: real_idx,
        remaining: 1,
        receiver: mk(9),
        collected: 1,
        owner: accounts(1),
        escrow_shares: 0,
    };

    let call_op = if pass_op { real_op } else { real_op + 1 };
    let call_idx = if pass_index { real_idx } else { real_idx + 1 };

    let r = c.after_exec_withdraw_read(Ok(None), call_op, call_idx, U128(10), U128(1));
    if let (true, true) = (pass_op, pass_index) {
        assert!(
            !matches!(c.op_state, OpState::Idle),
            "Valid callback should not immediately stop"
        );
    } else {
        // Any mismatch should stop and go Idle
        if let PromiseOrValue::Value(()) = r {}
        assert!(
            matches!(c.op_state, OpState::Idle),
            "Mismatched callback must stop and go Idle"
        );
    }
}

#[test]
fn refund_path_consistency() {
    use near_sdk_contract_tools::ft::Nep141Controller as _;

    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Seed escrowed shares into the vault's own account
    let owner = accounts(1);
    c.deposit_unchecked(&near_sdk::env::current_account_id(), 10)
        .unwrap_or_else(|e| near_sdk::env::panic_str(&e.to_string()));

    // Single-market withdraw queue (not used functionally here, just to satisfy path)
    let market = mk(12);
    c.withdraw_queue.push(market);

    // Withdrawing state with remaining=0 and collected=0 forces refund path
    c.op_state = OpState::Withdrawing {
        op_id: 77,
        index: 0,
        remaining: 0,
        receiver: mk(9),
        collected: 0,
        owner: owner.clone(),
        escrow_shares: 10,
    };

    let supply_before = c.total_supply();
    let vault_before = c.balance_of(&near_sdk::env::current_account_id());
    let owner_before = c.balance_of(&owner);

    // Read result with need=0 ensures credited=0; triggers refund branch
    let res = c.after_exec_withdraw_read(Ok(None), 77, 0, U128(0), U128(0));
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) on immediate escrow refund"),
    }

    // No burn/mint => total supply unchanged
    assert_eq!(
        c.total_supply(),
        supply_before,
        "no supply change on refund"
    );
    // Escrow shares transferred back to owner
    assert_eq!(
        c.balance_of(&near_sdk::env::current_account_id()),
        vault_before.saturating_sub(10),
        "vault should lose refunded escrow"
    );
    assert_eq!(
        c.balance_of(&owner),
        owner_before.saturating_add(10),
        "owner should receive refunded escrow"
    );
    // Vault returns to Idle
    assert!(
        matches!(c.op_state, OpState::Idle),
        "Vault must go Idle after refund"
    );
}

#[test]
fn ctx_allocating_ok_and_err() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    c.op_state = OpState::Allocating {
        op_id: 42,
        index: 3,
        remaining: 77,
    };

    let ok = c.ctx_allocating(42).expect("ctx_allocating should succeed");
    assert_eq!(ok, (3, 77));

    // Wrong op_id => error
    assert!(c.ctx_allocating(43).is_err());
}

#[test]
fn ctx_withdrawing_ok_and_err() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let recv = mk(1);
    let owner = accounts(1);

    c.op_state = OpState::Withdrawing {
        op_id: 7,
        index: 1,
        remaining: 50,
        receiver: recv.clone(),
        collected: 5,
        owner: owner.clone(),
        escrow_shares: 10,
    };

    let (idx, rem, r, coll, o, escrow) = c
        .ctx_withdrawing(7)
        .expect("ctx_withdrawing should succeed");
    assert_eq!(idx, 1);
    assert_eq!(rem, 50);
    assert_eq!(r, recv);
    assert_eq!(coll, 5);
    assert_eq!(o, owner);
    assert_eq!(escrow, 10);

    // Wrong op_id => error
    assert!(c.ctx_withdrawing(8).is_err());
}

#[test]
fn resolve_market_helpers_supply_and_withdraw() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Prepare markets
    let m1 = mk(1001);
    let m2 = mk(1002);

    // Supply: plan takes precedence
    c.plan = Some(vec![(m2.clone(), 1u128)]);
    c.supply_queue.push(m1.clone());
    c.supply_queue.push(m2.clone());

    assert_eq!(c.resolve_supply_market(0).unwrap(), m2);
    assert!(matches!(
        c.resolve_supply_market(1),
        Err(Error::MissingMarket(1))
    ));

    // Without plan, use queue
    c.plan = None;
    assert_eq!(c.resolve_supply_market(0).unwrap(), m1);
    assert_eq!(c.resolve_supply_market(1).unwrap(), m2);
    assert!(matches!(
        c.resolve_supply_market(2),
        Err(Error::MissingMarket(2))
    ));

    // Withdraw resolver uses withdraw_queue
    c.withdraw_queue.push(m1.clone());
    c.withdraw_queue.push(m2.clone());
    assert_eq!(c.resolve_withdraw_market(0).unwrap(), m1);
    assert_eq!(c.resolve_withdraw_market(1).unwrap(), m2);
    assert!(matches!(
        c.resolve_withdraw_market(2),
        Err(Error::MissingMarket(2))
    ));
}

#[test]
fn after_supply_2_read_missing_position_stops() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Resolve market via supply_queue
    let market = mk(42);
    c.supply_queue.push(market);

    // Must be in Allocating ctx
    c.op_state = OpState::Allocating {
        op_id: 1,
        index: 0,
        remaining: 10,
    };

    // Missing position -> stop_and_exit
    let res = c.after_supply_2_read(Ok(None), 1, 0, U128(0), U128(5), U128(5));
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value on missing position"),
    }
    assert!(matches!(c.op_state, OpState::Idle));
}

#[test]
fn after_supply_2_read_read_failed_stops() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Resolve market via supply_queue
    let market = mk(43);
    c.supply_queue.push(market);

    // Must be in Allocating ctx
    c.op_state = OpState::Allocating {
        op_id: 7,
        index: 0,
        remaining: 100,
    };

    // Read failure -> stop_and_exit
    let res = c.after_supply_2_read(
        Err(near_sdk::PromiseError::Failed),
        7,
        0,
        U128(0),
        U128(10),
        U128(10),
    );
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value on read failure"),
    }
    assert!(matches!(c.op_state, OpState::Idle));
}

#[rstest]
fn after_create_withdraw_req_success_returns_promise(
    mut c: Contract,
    receiver: AccountId,
    owner: AccountId,
) {
    let market = mk(50);
    c.withdraw_queue.push(market.clone());
    c.market_supply.insert(market.clone(), 100);

    c.op_state = OpState::Withdrawing {
        op_id: 21,
        index: 0,
        remaining: 60,
        receiver: receiver.clone(),
        collected: 10,
        owner: owner.clone(),
        escrow_shares: 5,
    };

    let res = c.after_create_withdraw_req(Ok(()), 21, 0, U128(60));
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise when create succeeds"),
    }
    // State remains Withdrawing and will continue via the promise chain
    assert!(matches!(c.op_state, OpState::Withdrawing { .. }));
}

#[rstest]
fn after_exec_withdraw_req_returns_promise(mut c: Contract) {
    let market = mk(60);
    c.withdraw_queue.push(market.clone());
    c.market_supply.insert(market.clone(), 10);

    c.op_state = OpState::Withdrawing {
        op_id: 33,
        index: 0,
        remaining: 5,
        receiver: mk(9),
        collected: 0,
        owner: accounts(1),
        escrow_shares: 0,
    };

    let res = c.after_exec_withdraw_req(33, 0, U128(5));
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise to read supply position after exec"),
    }
    assert!(matches!(c.op_state, OpState::Withdrawing { .. }));
}

#[rstest]
fn after_exec_withdraw_read_advances_when_remaining(
    mut c: Contract,
    owner: AccountId,
    receiver: AccountId,
) {
    // Two markets; first has principal to withdraw
    let m1 = mk(70);
    let m2 = mk(71);
    c.withdraw_queue.push(m1.clone());
    c.withdraw_queue.push(m2.clone());
    c.market_supply.insert(m1.clone(), 10);

    c.op_state = OpState::Withdrawing {
        op_id: 0,
        index: 0,
        remaining: 100,
        receiver: receiver.clone(),
        collected: 0,
        owner: owner.clone(),
        escrow_shares: 0,
    };

    // Position None => new_principal = 0 => withdrawn = 10 => credited = 10
    let res = c.after_exec_withdraw_read(Ok(None), 0, 0, U128(10), U128(100));
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise to continue withdraw steps"),
    }

    // Idle credited, state advanced to next index with remaining reduced
    assert_eq!(c.idle_balance, 10);

    // This works
    match &c.op_state {
        OpState::Payout {
            op_id,
            receiver: r,
            amount,
            owner: o,
            escrow_shares,
            burn_shares,
        } => {
            assert_eq!(*op_id, 0);
            assert_eq!(*amount, 10);
            assert_eq!(*escrow_shares, 0);
            assert_eq!(*burn_shares, 0);
            assert_eq!(*r, receiver);
            assert_eq!(*o, owner);
        }
        other => panic!("Unexpected state after advancing: {other:?}"),
    }
}

#[rstest]
fn stop_and_exit_when_idle_emits_and_stays_idle(mut c: Contract) {
    // Already Idle; ensure branch is executed
    c.op_state = OpState::Idle;

    let res = c.stop_and_exit::<&str>(Some(&"reason"));
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value on stop while Idle"),
    }
    assert!(matches!(c.op_state, OpState::Idle));
}
#[test]
fn accepts_increase_and_decrements_remaining() {
    let out = reconcile_supply_outcome(&1_600, &1_000, &1_000);
    let expected_accepted = 1_600u128.saturating_sub(1_000);
    let expected_remaining = 1_000u128.saturating_sub(expected_accepted);

    assert_eq!(out.new_principal, 1_600);
    assert_eq!(out.accepted_event, expected_accepted); // 600
    assert_eq!(out.remaining, expected_remaining); // 400
}

#[test]
fn no_accept_when_total_does_not_increase() {
    // decreased
    let out = reconcile_supply_outcome(&1_500, &2_000, &5_000);
    assert_eq!(out.new_principal, 1_500);
    assert_eq!(out.accepted_event, 0);
    assert_eq!(out.remaining, 5_000);

    // equal
    let out = reconcile_supply_outcome(&2_000, &2_000, &1_234);
    assert_eq!(out.new_principal, 2_000);
    assert_eq!(out.accepted_event, 0);
    assert_eq!(out.remaining, 1_234);
}

#[test]
fn remaining_saturates_to_zero_when_acceptance_exceeds_it() {
    let out = reconcile_supply_outcome(&u128::MAX, &0, &1);
    assert_eq!(out.new_principal, u128::MAX);
    assert_eq!(out.accepted_event, u128::MAX);
    assert_eq!(out.remaining, 0);

    let out = reconcile_supply_outcome(&10_000, &0, &5);
    assert_eq!(out.new_principal, 10_000);
    assert_eq!(out.accepted_event, 10_000);
    assert_eq!(out.remaining, 0);
}

#[test]
fn handles_extreme_boundaries_correctly() {
    let out = reconcile_supply_outcome(&0, &0, &0);
    assert_eq!(out.new_principal, 0);
    assert_eq!(out.accepted_event, 0);
    assert_eq!(out.remaining, 0);

    let out = reconcile_supply_outcome(&0, &u128::MAX, &123);
    assert_eq!(out.new_principal, 0);
    assert_eq!(out.accepted_event, 0);
    assert_eq!(out.remaining, 123);

    let out = reconcile_supply_outcome(&u128::MAX, &(u128::MAX - 5), &2);
    assert_eq!(out.new_principal, u128::MAX);
    assert_eq!(out.accepted_event, 5);
    assert_eq!(out.remaining, 0);
}

#[rstest]
fn stop_and_exit_payout_refunds_and_idle(mut c: Contract, owner: AccountId, receiver: AccountId) {
    use near_sdk_contract_tools::ft::Nep141Controller as _;
    let escrow: u128 = 10;

    // Seed escrowed shares into the vault's own account
    c.deposit_unchecked(&near_sdk::env::current_account_id(), escrow)
        .unwrap_or_else(|e| near_sdk::env::panic_str(&e.to_string()));

    // Enter Payout with non-zero escrow
    c.op_state = OpState::Payout {
        op_id: 123,
        receiver: receiver.clone(),
        amount: 77,
        owner: owner.clone(),
        escrow_shares: escrow,
        burn_shares: escrow,
    };

    let supply_before = c.total_supply();
    let vault_before = c.balance_of(&near_sdk::env::current_account_id());
    let owner_before = c.balance_of(&owner);
    let idle_before = c.idle_balance;

    c.stop_and_exit_payout::<&str>(Some(&"reason"));

    // Escrow refunded, no burn, vault goes Idle
    assert!(matches!(c.op_state, OpState::Idle));
    assert_eq!(c.total_supply(), supply_before, "No burn/mint on stop");
    assert_eq!(
        c.balance_of(&near_sdk::env::current_account_id()),
        vault_before.saturating_sub(escrow),
        "Vault should transfer escrow to owner"
    );
    assert_eq!(
        c.balance_of(&owner),
        owner_before.saturating_add(escrow),
        "Owner should receive escrow refund"
    );
    assert_eq!(c.idle_balance, idle_before, "Idle balance unchanged");
}

#[rstest]
fn stop_and_exit_payout_zero_escrow_just_idle(
    mut c: Contract,
    owner: AccountId,
    receiver: AccountId,
) {
    // Enter Payout with zero escrow; no transfers should occur
    c.op_state = OpState::Payout {
        op_id: 7,
        receiver,
        amount: 1,
        owner: owner.clone(),
        escrow_shares: 0,
        burn_shares: 0,
    };

    let supply_before = c.ft_total_supply();
    let vault_before = c.ft_balance_of(near_sdk::env::current_account_id());
    let owner_before = c.ft_balance_of(owner.clone());

    c.stop_and_exit_payout::<&str>(None);

    assert!(matches!(c.op_state, OpState::Idle));
    assert_eq!(c.ft_total_supply(), supply_before, "No supply change");
    assert_eq!(
        c.ft_balance_of(near_sdk::env::current_account_id()),
        vault_before,
        "Vault balance unchanged"
    );
    assert_eq!(
        c.ft_balance_of(owner),
        owner_before,
        "Owner balance unchanged"
    );
}
