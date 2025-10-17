use crate::storage_management::storage_bytes_for_queue_account_id;
use crate::storage_management::yocto_for_bytes;
use crate::storage_management::yocto_for_new_market;
use crate::test_utils::*;
use crate::Contract;
use near_sdk::env;
use near_sdk::test_utils::accounts;
use near_sdk::{json_types::U128, AccountId};
use near_sdk_contract_tools::ft::Nep141Controller as _;
use near_sdk_contract_tools::owner::OwnerExternal;
use rstest::rstest;
use templar_common::vault::MarketConfiguration;
use templar_common::vault::OpState;

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
    c.deposit_unchecked(&user, 1_000).unwrap_or_else(|e| env::panic_str(&e.to_string()));
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
fn payout_success_burns_only_proportional_escrow_and_refunds_remainder() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

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

#[test]
fn execute_next_withdrawal_request_skips_holes() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap_or_else(|| env::panic_str(&"Owner not set".to_string()));
    setup_env(&vault_id, &owner, vec![]);

    println!("vault_id: {vault_id}");
    println!("owner: {owner}");

    // Bob gets 20 shares
    c.deposit_unchecked(&owner, 20).unwrap_or_else(|e| env::panic_str(&e.to_string()));
    // We fake by adding idle to the vault
    c.transfer_unchecked(&owner, &vault_id, 10).unwrap_or_else(|e| env::panic_str(&e.to_string()));
    c.transfer_unchecked(&owner, &vault_id, 10).unwrap_or_else(|e| env::panic_str(&e.to_string()));

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
    setup_env(&vault_id, &c.own_get_owner().unwrap_or_else(|| env::panic_str(&"Owner not set".to_string())), vec![]);

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

#[test]
fn start_allocation_reserves_only_amount() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Configure a single market with cap = 80 in the supply queue
    let m1 = mk(2000);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = 80;
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
    setup_env(&vault_id, &c.own_get_owner().unwrap_or_else(|| env::panic_str(&"Owner not set".to_string())), vec![]);

    // Supply queue has m1; stale plan points to m2
    let m1 = mk(3001);
    let m2 = mk(3002);

    let mut cfg1 = MarketConfiguration::default();
    cfg1.cap = 10;
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
    setup_env(&vault_id, &c.own_get_owner().unwrap_or_else(|| env::panic_str(&"Owner not set".to_string())), vec![]);

    let m1 = mk(4001);

    let mut cfg = MarketConfiguration::default();
    cfg.cap = 0; // required precondition to attempt removal
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
    let c = new_test_contract(&vault_id);

    let cur = 1_500u128;
    let last = 1_000u128;
    let perf = crate::wad::WAD / 10; // 10%
    let ts = 1_000u128;
    let vs = 1u128;
    let va = 1u128;

    let (nts, nta) = Contract::compute_effective_totals(cur, last, perf, ts, vs, va);
    let expected_fee = crate::wad::compute_fee_shares(cur, last, perf, ts);

    assert_eq!(nts, ts + expected_fee + vs);
    assert_eq!(nta, cur + va);
}

#[test]
fn compute_escrow_settlement_burns_min_and_refunds_rest() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let c = new_test_contract(&vault_id);

    assert_eq!(Contract::compute_escrow_settlement(100, 40), (40, 60));
    assert_eq!(Contract::compute_escrow_settlement(100, 200), (100, 0));
    assert_eq!(Contract::compute_escrow_settlement(0, 50), (0, 0));
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
    cfg.cap = 0;
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
    cfg.cap = 10;
    cfg.enabled = true;
    c.config.insert(m.clone(), cfg);

    // Lower cap to zero: should NOT disable the market anymore
    c.submit_cap(m.clone(), U128(0));
    let cfg_after = c.config.get(&m).expect("market must exist");
    assert_eq!(cfg_after.cap, 0, "cap must be updated to 0");
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
    assert_eq!(cfg1.cap, raise);
    assert!(cfg1.enabled, "market should be enabled after raise");
    assert!(
        c.withdraw_queue.iter().any(|x| x == &m),
        "market must be in withdraw queue after enabling"
    );

    // Now lower back to 0 (immediate path) and ensure enabled stays true
    c.submit_cap(m.clone(), U128(0));
    let cfg2 = c.config.get(&m).unwrap();
    assert_eq!(cfg2.cap, 0);
    assert!(cfg2.enabled, "enabled must remain true on cap=0");
}

#[test]
#[should_panic = "Policy violation: Cannot remove market with non-zero cap"]
fn set_withdraw_queue_disallow_nonzero_cap_removal() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &c.own_get_owner().unwrap_or_else(|| env::panic_str(&"Owner not set".to_string())), vec![]);

    let m = mk(5000);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = 1; // non-zero cap
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
    cfg.cap = 0;
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
    cfg.cap = 0;
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
    setup_env(&vault_id, &c.own_get_owner().unwrap_or_else(|| env::panic_str(&"Owner not set".to_string())), vec![]);

    let m = mk(5003);
    let mut cfg = MarketConfiguration::default();
    cfg.cap = 0;
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
    let c = new_test_contract(&mk(0));
    let (credited, remaining_next, collected_next, idle_delta) =
        c.reconcile_withdraw_outcome(before, new_principal, need, rem, coll);

    let withdrawn = before.saturating_sub(new_principal);
    let expected_credited = withdrawn.min(need);

    assert_eq!(credited, expected_credited);
    assert!(credited <= need);
    assert_eq!(remaining_next, rem.saturating_sub(credited));
    assert_eq!(collected_next, coll.saturating_add(credited));
    assert_eq!(idle_delta, credited);
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
    cfg.cap = cap;
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
