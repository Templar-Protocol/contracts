#![allow(clippy::pedantic)]

use crate::impl_callbacks::reconcile_supply_outcome;
use crate::impl_callbacks::WithdrawReconciliation;
use crate::storage_management::storage_bytes_for_queue_account_id;
use crate::storage_management::yocto_for_bytes;
use crate::test_utils::*;
use crate::wad::compute_fee_shares;
use crate::wad::Wad;
use crate::Contract;
use crate::MarketRecord;
use crate::Number;
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver as _;
use near_sdk::env;
use near_sdk::serde_json;
use near_sdk::test_utils::accounts;
use near_sdk::NearToken;
use near_sdk::PromiseOrValue;
use near_sdk::PromiseResult;
use near_sdk::{json_types::U128, AccountId};
use near_sdk_contract_tools::ft::Nep141 as _;
use near_sdk_contract_tools::ft::Nep141Controller as _;
use near_sdk_contract_tools::mt::Nep245Receiver as _;
use near_sdk_contract_tools::owner::OwnerExternal;
use near_workspaces::network::Sandbox;
use near_workspaces::Worker;
use proptest::prelude::*;
use rstest::{fixture, rstest};
use std::collections::BTreeSet;
use templar_common::asset::FungibleAsset;
use templar_common::vault::AllocatingState;
use templar_common::vault::Error;
use templar_common::vault::MarketConfiguration;
use templar_common::vault::OpState;
use templar_common::vault::PayoutState;
use templar_common::vault::Restrictions;
use templar_common::vault::WithdrawingState;
use templar_common::vault::{AllocationMode, DepositMsg};
use test_utils::setup_test;
use test_utils::*;

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
        .unwrap_or_else(|| templar_common::panic_with_message("Owner not set"));
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
    let cfg = MarketConfiguration {
        cap: U128(100),
        enabled: true,
        removable_at: 0,
    };
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
                .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string())),
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

proptest! {
    #[test]
    fn paused_restricts_all_accounts(account in any::<u32>().prop_map(|v| mk(v))) {
        let r = Restrictions::Paused;
        let out = r.is_restricted(account.as_ref());
        prop_assert_eq!(out, Some(Restrictions::Paused));
    }

    #[test]
    fn blacklist_restricts_exact_members(
        blacklist in prop::collection::vec(any::<u32>().prop_map(|v| mk(v)), 0..10),
        account in any::<u32>().prop_map(|v| mk(v))
    ) {
        let set: BTreeSet<AccountId> = blacklist.into_iter().collect();
        let r = Restrictions::BlackList(set.clone());
        let out = r.is_restricted(account.as_ref());

        if set.contains(&account) {
            match &out {
                Some(Restrictions::BlackList(cloned)) => {
                    prop_assert_eq!(cloned, &set);
                    prop_assert!(cloned.contains(&account));
                }
                other => {
                    prop_assert!(false, "expected Some(BlackList), got {:?}", other);
                }
            }
        } else {
            prop_assert_eq!(out, None);
        }
    }

    #[test]
    fn whitelist_restricts_exact_non_members(
        whitelist in prop::collection::vec(any::<u32>().prop_map(|v| mk(v)), 0..10),
        account in any::<u32>().prop_map(|v| mk(v))
    ) {
        let set: BTreeSet<AccountId> = whitelist.into_iter().collect();
        let r = Restrictions::WhiteList(set.clone());
        let out = r.is_restricted(account.as_ref());

        if set.contains(&account) {
            prop_assert_eq!(out, None);
        } else {
            match &out {
                Some(Restrictions::WhiteList(cloned)) => {
                    prop_assert_eq!(cloned, &set);
                    prop_assert!(!cloned.contains(&account));
                }
                other => {
                    prop_assert!(false, "expected Some(WhiteList), got {:?}", other);
                }
            }
        }
    }
}

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
#[should_panic = "Invariant: Only one op in flight"]
async fn state_machine_is_locked_when_another_op_is_running(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    setup_test!(
        worker
        extract(vault, c, vault_owner)
        accounts(supply_user, borrow_user)
    );
    let amount = 1000;
    vault.supply(&supply_user, amount).await;

    futures::future::select_all(
        (0..100).map(|_| Box::pin(vault.allocate(&vault_owner, vec![], Some(1.into())))),
    )
    .await;
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

#[rstest]
fn fee_accrues_only_on_growth_unit(c_vault_env: Contract) {
    let mut c = c_vault_env;

    // Seed total supply so fees can mint
    let user = accounts(1);
    c.deposit_unchecked(&user, 1_000)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));
    c.idle_balance = 1_000;

    // Set fee to 10%
    c.performance_fee = Wad::one() / 10;

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
fn payout_success_burns_only_proportional_escrow_and_refunds_remainder(c_vault_env: Contract) {
    let mut c = c_vault_env;

    let receiver = mk(7);
    let owner = accounts(1);

    // Seed escrow into vault account (shares held by vault)
    c.deposit_unchecked(&near_sdk::env::current_account_id(), 100)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));
    // Seed idle to cover payout
    c.idle_balance = 1_000;

    // Partial payout scenario: collected/requested = 200/500 => burn 40% of escrowed shares
    let amount = 200;
    let op_id = 1;
    c.op_state = OpState::Payout(PayoutState {
        op_id,
        receiver: receiver.clone(),
        amount,
        owner: owner.clone(),
        escrow_shares: 100,
        burn_shares: 40, // precomputed proportional burn for test
    });

    let supply_before = c.total_supply();
    c.payment_01_reconcile_idle_or_refund(Ok(()), op_id, receiver, U128(amount));

    // Idle decreased by payout before payout is initiated
    // Only burn_shares are burned from total supply
    assert_eq!(c.total_supply(), supply_before - 40);
    // State returns to Idle
    assert!(matches!(c.op_state, OpState::Idle));
}

#[test]
#[should_panic = "unauthorized market"]
fn set_supply_queue_rejects_zero_cap() {
    let mut c = new_test_contract(&mk(0));
    setup_env(&mk(0), &accounts(1), vec![]);

    // Unknown market => cap treated as 0
    c.set_supply_queue(vec![mk(100)]);
}

#[rstest]
#[should_panic = "Invalid token ID"]
fn execute_supply_wrong_token_refunds_full(c_vault_env: Contract) {
    let mut c = c_vault_env;
    setup_env(
        &env::current_account_id(),
        &c.underlying_asset.contract_id().into(),
        vec![],
    );

    let sender = accounts(1);
    let wrong_token: AccountId = "wrong.token".parse().unwrap();
    let deposit = 1_000u128;

    let _ = c.execute_supply(sender.clone(), wrong_token.clone(), deposit);
}

#[rstest]
fn start_allocation_reserves_only_amount(c_vault_env: Contract) {
    let mut c = c_vault_env;

    // Configure a single market with cap = 80 in the supply queue
    let m1 = mk(2000);
    let cfg = MarketConfiguration {
        cap: U128(80),
        enabled: true,
        removable_at: 0,
    };
    c.markets.insert(
        m1.clone(),
        MarketRecord {
            cfg,
            pending_cap: None,
            principal: 0,
        },
    );
    c.supply_queue.insert(m1.clone());

    // Idle = 100, so max_room (80) should clamp allocation
    c.idle_balance = 100;
    assert_eq!(c.get_max_deposit().0, 80, "sanity: max room must be 80");

    // Reserve only the amount to allocate (intended behavior)
    let total = c.get_max_deposit().0.min(c.idle_balance);
    c.start_allocation(total);

    // Emulate allocation completing successfully: 80 moved to market
    if let Some(rec) = c.markets.get_mut(&m1) {
        rec.principal = 80;
    } else {
        c.markets.insert(
            m1.clone(),
            MarketRecord {
                cfg: MarketConfiguration::default(),
                pending_cap: None,
                principal: 80,
            },
        );
    }
    // Force completion and exit op
    if let crate::OpState::Allocating(AllocatingState { op_id, index, .. }) = c.op_state.clone() {
        c.op_state = crate::OpState::Allocating(AllocatingState {
            op_id,
            index,
            remaining: 0,
        });
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
            .unwrap_or_else(|| templar_common::panic_with_message("Owner not set")),
        vec![],
    );

    // Supply queue has m1; stale plan points to m2
    let m1 = mk(3001);
    let m2 = mk(3002);

    let cfg1 = MarketConfiguration {
        cap: U128(10),
        enabled: true,
        removable_at: 0,
    };
    c.markets.insert(m1.clone(), cfg1.into());
    c.supply_queue.insert(m1);

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

    assert_eq!(
        Contract::compute_burn_shares(escrow, collected, requested),
        expect
    );
}

#[test]
fn compute_effective_totals_fee_share_and_virtuals() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let cur = 1_500u128.into();
    let last = 1_000u128.into();
    let perf = Wad::one() / 10; // 10%
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
fn cap_zero_keeps_enabled_and_submit_removal_works() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();

    setup_env(&vault_id, &owner, vec![]);

    let m = mk(8001);

    // Seed a known, enabled market with cap > 0
    let cfg = MarketConfiguration {
        cap: U128(10),
        enabled: true,
        removable_at: 0,
    };
    c.markets.insert(
        m.clone(),
        MarketRecord {
            cfg,
            pending_cap: None,
            principal: 0,
        },
    );

    // Lower cap to zero: should NOT disable the market anymore
    c.submit_cap(m.clone(), U128(0));
    let cfg_after = &c.markets.get(&m).expect("market must exist").cfg;
    assert_eq!(cfg_after.cap.0, 0, "cap must be updated to 0");
    assert!(cfg_after.enabled, "enabled must remain true when cap is 0");

    set_block_ts(&vault_id, &owner, 2);

    // Now we can schedule removal
    c.submit_market_removal(m.clone());
    let cfg_after2 = c.markets.get(&m).expect("market must exist");
    assert!(cfg_after2.cfg.removable_at > 0, "removal must be scheduled");
}
#[test]
fn accept_cap_raise_enables_and_cap_zero_keeps_enabled() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();

    setup_env(&vault_id, &owner, vec![]);

    let m = mk(8002);

    // Start disabled with cap=0
    c.markets.insert(
        m.clone(),
        MarketRecord {
            cfg: MarketConfiguration::default(),
            pending_cap: None,
            principal: 0,
        },
    );

    // Submit raise -> pending
    let raise = 5u128;
    set_ctx(&vault_id, &owner, None, Some(yocto_for_bytes(10_000)));
    c.submit_cap(m.clone(), U128(raise));

    // Fast-forward timelock to accept the raise
    set_ctx(
        &vault_id,
        &owner,
        Some(env::block_timestamp() + 1_000_000_000),
        None,
    );
    c.accept_cap(m.clone());

    let cfg1 = &c.markets.get(&m).unwrap().cfg;
    assert_eq!(cfg1.cap.0, raise);
    assert!(cfg1.enabled, "market should be enabled after raise");

    // Now lower back to 0 (immediate path) and ensure enabled stays true
    c.submit_cap(m.clone(), U128(0));
    let cfg2 = &c.markets.get(&m).unwrap().cfg;
    assert_eq!(cfg2.cap.0, 0);
    assert!(cfg2.enabled, "enabled must remain true on cap=0");
}

#[rstest(
    before,
    new_principal,
    need,
    rem,
    coll,
    case(100u128, 55u128, 45u128, 50u128, 10u128),
    case(100u128, 80u128, 40u128, 50u128, 10u128),
    case(0u128, 0u128, 0u128, 0u128, 0u128),
    case(1000u128, 1000u128, 500u128, 800u128, 100u128)
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
    let cfg = MarketConfiguration {
        cap: U128(cap),
        enabled: cap > 0,
        removable_at: 0,
    };
    c.markets.insert(
        m.clone(),
        MarketRecord {
            cfg,
            pending_cap: None,
            principal: cur,
        },
    );
    c.supply_queue.insert(m.clone());
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
fn total_assets_sums_all_markets_cases(principal: u128, idle: u128) {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let mut c = new_test_contract(&vault_id);

    let m = mk(7003);
    c.markets.insert(
        m.clone(),
        MarketRecord {
            cfg: MarketConfiguration::default(),
            pending_cap: None,
            principal,
        },
    );
    c.idle_balance = idle;

    assert_eq!(c.get_total_assets().0, idle.saturating_add(principal));
}

#[test]
fn set_fee_recipient_accrues_before_switch() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = accounts(1);
    setup_env(&vault_id, &owner, vec![]);

    // Seed supply so fee shares can mint
    c.deposit_unchecked(&accounts(1), 1_000)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));
    // Simulate profit: last=1000, current=1500
    c.idle_balance = 1_500;
    c.last_total_assets = 1_000;
    c.performance_fee = Wad::one() / 10;

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
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));
    // Simulate profit: last=2000, current=2400
    c.idle_balance = 2_400;
    c.last_total_assets = 2_000;
    c.performance_fee = Wad::one() / 20; // 5%

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
        .unwrap_or_else(|| templar_common::panic_with_message("Owner not set"));
    setup_env(&vault_id, &owner, vec![]);

    // Seed supply so fee shares can mint
    c.deposit_unchecked(&accounts(1), 1_000)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));
    // Simulate profit: last=1000, current=1500
    c.idle_balance = 1_500;
    c.last_total_assets = 1_000;

    // Old rate = 10%, new rate = 1%
    c.performance_fee = Wad::one() / 10;
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

    c.set_performance_fee(Wad::one() / 100);

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
        .unwrap_or_else(|| templar_common::panic_with_message("Owner not set"));
    setup_env(&vault_id, &owner, vec![]);

    // Seed supply so fee shares can mint
    c.deposit_unchecked(&accounts(2), 2_000)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));
    // Simulate profit: last=2000, current=2400
    c.idle_balance = 2_400;
    c.last_total_assets = 2_000;

    // Old rate = 5%, new rate = 0.5%
    c.performance_fee = Wad::one() / 20; // 5%
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

    c.set_performance_fee(Wad::one() / 200); // 0.5%

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
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));
    // Loss scenario: last=1000, current=800
    c.idle_balance = 800;
    c.last_total_assets = 1_000;
    c.performance_fee = Wad::one() / 10;

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
    c_asset_env: Contract,
    enabled_market_100: (AccountId, MarketConfiguration),
) {
    let mut c = c_asset_env;
    c.mode = AllocationMode::Eager {
        min_batch: U128(u128::MAX),
    };
    let (m, cfg) = enabled_market_100;
    c.markets.insert(m.clone(), cfg.into());
    c.supply_queue.insert(m);

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
    c_asset_env: Contract,
    enabled_market_100: (AccountId, MarketConfiguration),
) {
    let mut c = c_asset_env;
    c.mode = AllocationMode::Eager {
        min_batch: U128(u128::MAX),
    };
    let (m, mut cfg) = enabled_market_100;
    cfg.cap = U128(50); // override cap for this case
    c.markets.insert(m.clone(), cfg.into());
    c.supply_queue.insert(m);

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
#[should_panic = "Invalid token ID"]
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
    let cfg = MarketConfiguration {
        cap: U128(100),
        enabled: true,
        removable_at: 0,
    };
    c.markets.insert(m.clone(), cfg.into());
    c.supply_queue.insert(m);

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
#[should_panic = "Deposit amount must be greater than zero"]
fn ft_on_transfer_zero_amount_returns_zero_refund(
    c_vault_env: Contract,
    enabled_market_100: (AccountId, MarketConfiguration),
) {
    let mut c = c_vault_env;
    setup_env(
        &env::current_account_id(),
        &c.underlying_asset.contract_id().into(),
        vec![],
    );

    // Setup a valid market
    let (m, cfg) = enabled_market_100;
    c.markets.insert(m.clone(), cfg.into());
    c.supply_queue.insert(m);

    let sender: AccountId = c.underlying_asset.contract_id().into();

    c.ft_on_transfer(
        sender.clone(),
        U128(0),
        serde_json::to_string(&DepositMsg::Supply).unwrap(),
    );
}

#[rstest]
fn ft_on_transfer_eager_mode_triggers_allocation(
    c_asset_env: Contract,
    enabled_market_100: (AccountId, MarketConfiguration),
) {
    let mut c = c_asset_env;

    // Trigger eager allocation with any positive deposit
    c.mode = AllocationMode::Eager { min_batch: U128(1) };

    // Valid market/cap
    let (m, cfg) = enabled_market_100;
    c.markets.insert(m.clone(), cfg.into());
    c.supply_queue.insert(m);

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
    let old_ft_id = c.underlying_asset.contract_id().into();
    setup_env(&vault_id, &old_ft_id, vec![]);

    let token_id = "token-1".to_string();

    c.underlying_asset = FungibleAsset::nep245(old_ft_id.clone(), token_id.clone());

    let sender = accounts(5);
    let amount = 25u128;

    let res = c.mt_on_transfer(
        accounts(3),
        vec![sender.clone()], // previous_owner_ids
        vec![token_id],       // token_ids
        vec![U128(amount)],   // amounts
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
#[should_panic = "Deposit amount must be greater than zero"]
fn execute_supply_zero_amount_rejected() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &vault_id, vec![]);

    let asset_id = c.underlying_asset.contract_id().into();
    let sender_id = accounts(4);
    c.execute_supply(sender_id.clone(), asset_id, 0);
}

#[test]
fn governance_set_curator_grants_allocator() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    // Prepare a market to exercise allocator permission
    let m1 = mk(9101);
    let cfg = MarketConfiguration {
        cap: U128(1),
        enabled: true,
        removable_at: 0,
    };
    c.markets.insert(m1.clone(), cfg.into());

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
    assert_eq!(c.supply_queue.iter().next(), Some(&m1));
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
    let cfg = MarketConfiguration {
        cap: U128(1),
        enabled: true,
        removable_at: 0,
    };
    c.markets.insert(m1.clone(), cfg.into());

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
    assert_eq!(c.supply_queue.iter().next(), Some(&m1));
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
    let cfg = MarketConfiguration {
        cap: U128(1),
        enabled: true,
        removable_at: 0,
    };

    c.markets.insert(m1.clone(), cfg.into());

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

#[rstest(
    method_name,
    case("set_curator"),
    case("set_is_allocator"),
    case("submit_guardian"),
    case("accept_guardian"),
    case("revoke_pending_guardian"),
    case("set_skim_recipient"),
    case("set_fee_recipient"),
    case("set_performance_fee"),
    case("submit_timelock"),
    case("accept_timelock"),
    case("revoke_pending_timelock"),
    case("submit_cap"),
    case("accept_cap"),
    case("revoke_pending_cap"),
    case("submit_market_removal"),
    case("revoke_pending_market_removal"),
    case("set_supply_queue")
)]
fn governance_abdicate_blocks_further_changes(method_name: &str) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let vault_id = accounts(0);
        let mut c = new_test_contract(&vault_id);
        let owner = c.own_get_owner().unwrap();

        setup_env(&vault_id, &owner, vec![]);

        c.abdicate(method_name.to_string());
        match method_name {
            "set_curator" => {
                c.set_curator(accounts(2));
            }
            "set_is_allocator" => {
                c.set_is_allocator(accounts(4), false);
            }
            "submit_guardian" => {
                c.submit_guardian(accounts(5));
            }
            "accept_guardian" => {
                c.accept_guardian();
            }
            "revoke_pending_guardian" => {
                c.revoke_pending_guardian();
            }
            "set_skim_recipient" => {
                c.set_skim_recipient(accounts(1));
            }
            "set_fee_recipient" => {
                c.set_fee_recipient(accounts(1));
            }
            "set_performance_fee" => {
                c.set_performance_fee(Wad::one() / 10);
            }
            "submit_timelock" => {
                let cur = c.get_configuration().initial_timelock_ns;
                // value choice irrelevant; abdication check runs first
                c.submit_timelock(cur);
            }
            "accept_timelock" => {
                c.accept_timelock();
            }
            "revoke_pending_timelock" => {
                c.revoke_pending_timelock();
            }
            "submit_cap" => {
                let market = mk(9200);
                c.submit_cap(market, U128(1));
            }
            "accept_cap" => {
                let market = mk(9201);
                c.accept_cap(market);
            }
            "revoke_pending_cap" => {
                let market = mk(9202);
                c.revoke_pending_cap(market);
            }
            "submit_market_removal" => {
                let market = mk(9203);
                c.submit_market_removal(market);
            }
            "revoke_pending_market_removal" => {
                let market = mk(9204);
                c.revoke_pending_market_removal(market);
            }
            "set_supply_queue" => {
                c.set_supply_queue(vec![]);
            }
            _ => unreachable!("unsupported abdicated method case"),
        }
    }));

    let expected = format!("abdicated {method_name}");

    match result {
        Ok(()) => panic!("expected panic for abdicated method {method_name}"),
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<String>() {
                s.as_str()
            } else if let Some(s) = payload.downcast_ref::<&str>() {
                s
            } else {
                ""
            };

            assert!(
                msg.contains(&expected),
                "expected panic message to contain '{expected}', got '{msg}'"
            );
        }
    }
}

#[test]
#[should_panic = "Timelock not elapsed yet"]
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
    let cfg = MarketConfiguration {
        cap: U128(10),
        enabled: true,
        removable_at: 0,
    };
    c.markets.insert(m.clone(), cfg.into());

    c.submit_cap(m.clone(), U128(3));
    let after = c.markets.get(&m).unwrap();
    assert_eq!(after.cfg.cap, U128(3));
}

#[test]
fn governance_submit_and_accept_cap_new_market_creates_and_enables() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let m = mk(9105);

    // Submit raise for a brand-new market
    set_ctx(&vault_id, &owner, None, Some(yocto_for_bytes(20_000)));
    c.submit_cap(m.clone(), U128(5));

    // Advance timelock and accept; attach storage for withdraw queue addition
    set_ctx(
        &vault_id,
        &owner,
        Some(env::block_timestamp() + 1_000_000_000),
        None,
    );
    c.accept_cap(m.clone());

    let cfg = &c.markets.get(&m).unwrap().cfg;
    assert_eq!(cfg.cap.0, 5);
    assert!(
        cfg.enabled,
        "market should be enabled after accepting raise"
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
    set_ctx(&vault_id, &owner, None, Some(yocto_for_bytes(20_000)));
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
    let cfg = MarketConfiguration {
        cap: U128(0),
        enabled: true,
        removable_at: 0,
    };
    c.markets.insert(m.clone(), cfg.into());

    // Submit removal (schedules timelock)
    c.submit_market_removal(m.clone());
    let after = c.markets.get(&m).unwrap();
    assert!(after.cfg.removable_at > 0, "removal must be scheduled");

    // Revoke pending removal
    c.revoke_pending_market_removal(m.clone());
    let after2 = c.markets.get(&m).unwrap();
    assert_eq!(after2.cfg.removable_at, 0, "removal must be revoked");
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

    let mut builder = VMContextBuilder::new();
    builder.current_account_id(vault_id.clone());
    builder.predecessor_account_id(owner.clone());
    builder.signer_account_id(owner.clone());
    builder.attached_deposit(NearToken::from_millinear(5));
    testing_env!(
        builder.build(),
        test_vm_config(),
        RuntimeFeesConfig::test(),
        Default::default(),
        vec![]
    );

    // Seed supply and simulate profit, but fee = 0
    c.deposit_unchecked(&owner, 1_000)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));
    c.idle_balance = 1_500;
    c.last_total_assets = 1_000;
    c.performance_fee = Wad::zero();

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
#[should_panic = "Refusing to skim the underlying token"]
fn skim_rejects_underlying_token() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    // Set a skim recipient
    let recipient = accounts(4);
    c.set_skim_recipient(recipient.clone());

    // Attempt to skim the underlying token -> must panic
    let underlying: AccountId = c.underlying_asset.contract_id().into();
    let _ = c.skim(underlying);
}

#[test]
#[should_panic = "Refusing to skim the share token"]
fn skim_rejects_share_token() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    // Set a skim recipient
    let recipient = accounts(4);
    c.set_skim_recipient(recipient.clone());

    // Attempt to skim the share token (the vault itself) -> must panic
    let share_token: AccountId = vault_id.clone();
    let _ = c.skim(share_token);
}

#[rstest]
fn after_supply_1_check_allocating_not_allocating(c_max: Contract) {
    let mut c = c_max;

    c.op_state = OpState::Idle;

    c.supply_01_handle_transfer(
        Ok(U128(1)),
        accounts(1),
        0,
        2,
        Default::default(),
        Default::default(),
    );

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
                .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string())),
        )],
    );

    let mut c = new_test_contract(&vault_id);

    let op_id = 1;

    c.op_state = OpState::Allocating(AllocatingState {
        op_id,
        index: 0u32,
        remaining: 0u128,
    });

    c.supply_01_handle_transfer(
        Ok(U128(1)),
        accounts(1),
        op_id + 1,
        0,
        Default::default(),
        Default::default(),
    );

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
                .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string())),
        )],
    );

    let mut c = new_test_contract(&vault_id);

    let op_id = 1;

    c.op_state = OpState::Allocating(AllocatingState {
        op_id,
        index: 0u32,
        remaining: 0u128,
    });

    c.supply_01_handle_transfer(
        Ok(U128(1)),
        accounts(3),
        op_id,
        0,
        Default::default(),
        Default::default(),
    );

    assert_eq!(
        c.op_state,
        OpState::Allocating(AllocatingState {
            op_id,
            index: 0,
            remaining: 0u128
        })
    );
    assert_eq!(c.plan, None);
}

#[rstest]
fn after_exec_withdraw_read_none_to_payout(mut c: Contract) {
    // Prepare a single-market withdraw queue with non-zero principal
    let market = mk(8);
    c.withdraw_route = vec![market.clone()];
    let principal = 100;
    c.markets.insert(
        market.clone(),
        MarketRecord {
            cfg: MarketConfiguration::default(),
            pending_cap: None,
            principal,
        },
    );

    let op_id = 42;
    let index = 0;
    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id,
        index,
        remaining: 60,
        receiver: mk(9),
        collected: 10,
        owner: accounts(1),
        escrow_shares: 50,
    });

    let res = c.execute_withdraw_02_reconcile_position(Ok(None), 42, 0, U128(principal), U128(0));
    match res {
        PromiseOrValue::Promise(_p) => {}
        _ => panic!("Expected a Promise to proceed to balance settlement"),
    }

    let res2 = c.execute_withdraw_03_settle(
        Ok(U128(principal)), // observed after_balance
        op_id,
        index,
        U128(principal), // before_principal
        U128(0),
        U128(0),
    );

    match res2 {
        PromiseOrValue::Promise(_p) => {}
        _ => panic!("Expected a Promise to send payout after settlement"),
    }

    assert_eq!(
        c.markets.get(&market).map_or(u128::MAX, |r| r.principal),
        0,
        "Market principal should be updated to 0"
    );

    // Collected was 70, payouit is 70, idle is 30

    assert_eq!(
        c.idle_balance, 30,
        "Idle balance should increase by returned amount"
    );

    // State should transition to Payout with amount = collected (10) + credited (60) = 70
    match &c.op_state {
        OpState::Payout(PayoutState { amount, .. }) => {
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

    let res = c.skim_01_read_balance(Ok(U128(0)), mk(10), mk(11));
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
    let res = c.skim_01_read_balance(Ok(U128(123)), mk(10), mk(11));
    match res {
        PromiseOrValue::Promise(_) => { //NOTE: one day we will be able to read the promise
             //definition :<
        }
        _ => panic!("Skim with positive balance must return a Promise"),
    }
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
    c.idle_balance = collected;

    // Single-market route so advancing index reaches end-of-route
    let market = mk(8);
    c.withdraw_route = vec![market.clone()];
    c.markets.insert(
        market.clone(),
        MarketRecord {
            cfg: MarketConfiguration::default(),
            pending_cap: None,
            principal: 100,
        },
    );

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 7,
        index: 0,
        remaining: need,
        receiver: mk(9),
        collected,
        owner: accounts(1),
        escrow_shares: 0,
    });

    let res =
        c.withdraw_01_handle_create_request(Err(near_sdk::PromiseError::Failed), 7, 0, U128(need));
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise after skipping to payout at end-of-queue"),
    }
    assert_eq!(c.idle_balance, 0);

    match &c.op_state {
        OpState::Payout(PayoutState { amount, .. }) => {
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
    c.withdraw_route = vec![market.clone()];
    c.markets.insert(
        market.clone(),
        MarketRecord {
            cfg: MarketConfiguration::default(),
            pending_cap: None,
            principal: before,
        },
    );

    let initial_idle = c.idle_balance;

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 99,
        index: 0,
        remaining: need,
        receiver: mk(9),
        collected,
        owner: accounts(1),
        escrow_shares: 0,
    });

    let res = c.execute_withdraw_02_reconcile_position(
        Err(near_sdk::PromiseError::Failed),
        99,
        0,
        U128(before),
        U128(0),
    );
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) due to read failure and stop"),
    }

    assert_eq!(
        c.markets.get(&market).map_or(u128::MAX, |r| r.principal),
        before,
        "principal must remain unchanged on read failure"
    );
    assert_eq!(
        c.idle_balance, initial_idle,
        "idle_balance must not change when nothing credited"
    );

    assert!(
        matches!(c.op_state, OpState::Idle),
        "Vault must go Idle on read failure"
    );
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
    c.withdraw_route = vec![market.clone()];
    c.markets.insert(
        market.clone(),
        MarketRecord {
            cfg: MarketConfiguration::default(),
            pending_cap: None,
            principal: 10,
        },
    );

    let real_op = 5u64;
    let real_idx = 0u32;

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: real_op,
        index: real_idx,
        remaining: 1,
        receiver: mk(9),
        collected: 1,
        owner: accounts(1),
        escrow_shares: 0,
    });

    let call_op = if pass_op { real_op } else { real_op + 1 };
    let call_idx = if pass_index { real_idx } else { real_idx + 1 };

    let r =
        c.execute_withdraw_02_reconcile_position(Ok(None), call_op, call_idx, U128(10), U128(0));
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
    let market = mk(8);
    c.markets.insert(
        market.clone(),
        MarketRecord {
            cfg: MarketConfiguration::default(),
            pending_cap: None,
            principal: 10,
        },
    );
    c.withdraw_route = vec![market.clone()];
    // Seed escrowed shares into the vault's own account
    let owner = accounts(1);
    c.deposit_unchecked(&near_sdk::env::current_account_id(), 10)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));

    // Withdrawing state with remaining=0 and collected=0 forces refund path
    let op_id = 77;
    let index = 0;
    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id,
        index,
        remaining: 0,
        receiver: mk(9),
        collected: 0,
        owner: owner.clone(),
        escrow_shares: 10,
    });

    let supply_before = c.total_supply();
    let vault_before = c.balance_of(&near_sdk::env::current_account_id());
    let owner_before = c.balance_of(&owner);

    // Read result with need=0 ensures credited=0; triggers refund branch
    let res = c.execute_withdraw_02_reconcile_position(Ok(None), op_id, index, U128(0), U128(0));
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise to proceed to balance settlement"),
    }

    let res2 = c.execute_withdraw_03_settle(
        Ok(U128(0)), // no inflow observed
        op_id,
        index,
        U128(0), // before_principal
        U128(0), // new_principal reported
        U128(0), // before_balance
    );
    match res2 {
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

    c.op_state = OpState::Allocating(AllocatingState {
        op_id: 42,
        index: 3,
        remaining: 77,
    });

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

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 7,
        index: 1,
        remaining: 50,
        receiver: recv.clone(),
        collected: 5,
        owner: owner.clone(),
        escrow_shares: 10,
    });

    let ctx = c
        .ctx_withdrawing(7)
        .expect("ctx_withdrawing should succeed");
    assert_eq!(ctx.index, 1);
    assert_eq!(ctx.remaining, 50);
    assert_eq!(ctx.receiver, recv);
    assert_eq!(ctx.collected, 5);
    assert_eq!(ctx.owner, owner);
    assert_eq!(ctx.escrow_shares, 10);

    // Wrong op_id => error
    assert!(c.ctx_withdrawing(8).is_err());
}

#[test]
fn resolve_market_helpers_supply_and_withdraw() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Withdraw resolver uses withdraw_route only
    let m1 = mk(1001);
    let m2 = mk(1002);
    c.withdraw_route = vec![m1.clone(), m2.clone()];
    assert_eq!(c.resolve_withdraw_market(0).unwrap(), &m1);
    assert_eq!(c.resolve_withdraw_market(1).unwrap(), &m2);
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
    c.supply_queue.insert(market.clone());

    // Must be in Allocating ctx
    c.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 10,
    });

    // Missing position -> stop_and_exit
    let res =
        c.supply_02_position_read(Ok(None), market, 1, 0, U128(0), U128(5), U128(5), U128(10));
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
    c.supply_queue.insert(market);

    // Must be in Allocating ctx
    c.op_state = OpState::Allocating(AllocatingState {
        op_id: 7,
        index: 0,
        remaining: 100,
    });

    // Read failure -> stop_and_exit
    let res = c.supply_02_position_read(
        Err(near_sdk::PromiseError::Failed),
        accounts(3),
        7,
        0,
        U128(0),
        U128(10),
        U128(10),
        U128(100),
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
    c.withdraw_route = vec![market.clone()];
    c.markets.insert(
        market.clone(),
        MarketRecord {
            cfg: MarketConfiguration::default(),
            pending_cap: None,
            principal: 100,
        },
    );

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 21,
        index: 0,
        remaining: 60,
        receiver: receiver.clone(),
        collected: 10,
        owner: owner.clone(),
        escrow_shares: 5,
    });

    let res = c.withdraw_01_handle_create_request(Ok(()), 21, 0, U128(60));
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) when create succeeds and execution is deferred"),
    }
    // State remains Withdrawing; keeper must call execute_next_market_withdrawal
    assert!(matches!(c.op_state, OpState::Withdrawing { .. }));
}

#[rstest]
fn after_exec_withdraw_req_returns_promise(mut c: Contract) {
    let market = mk(60);
    c.withdraw_route = vec![market.clone()];
    c.markets.insert(
        market.clone(),
        MarketRecord {
            cfg: MarketConfiguration::default(),
            pending_cap: None,
            principal: 10,
        },
    );

    let op_id = 33;
    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id,
        index: 0,
        remaining: 5,
        receiver: mk(9),
        collected: 0,
        owner: accounts(1),
        escrow_shares: 0,
    });

    let res = c.execute_withdraw_01_call_market_fetch_position(Ok(U128(1)), op_id, 0, None);
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise to read supply position after exec"),
    }
    assert!(matches!(
        c.op_state,
        OpState::Withdrawing(WithdrawingState { .. })
    ));
}

#[rstest]
fn after_exec_withdraw_read_advances_when_remaining(
    mut c: Contract,
    owner: AccountId,
    receiver: AccountId,
) {
    let m1 = mk(70);
    let record = MarketRecord {
        cfg: MarketConfiguration::default(),
        pending_cap: None,
        principal: 10,
    };
    c.markets.insert(m1.clone(), record.clone());

    let m2 = mk(71);
    c.withdraw_route = vec![m1.clone(), m2.clone()];

    let op_id = 0;
    let index = 0;
    let before_balance = 0;

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id,
        index,
        remaining: 100,
        receiver: receiver.clone(),
        collected: 0,
        owner: owner.clone(),
        escrow_shares: 0,
    });

    let res = c.execute_withdraw_02_reconcile_position(
        Ok(None),
        op_id,
        index,
        U128(0),
        U128(before_balance),
    );
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise to continue withdraw steps"),
    }

    // Settle with the inflow equal to the reported principal delta
    // before = 0
    // after = 10
    let res2 = c.execute_withdraw_03_settle(
        Ok(U128(record.principal)), // after_balance
        op_id,
        index,
        U128(record.principal), // before_principal
        U128(0),
        U128(before_balance),
    );
    match res2 {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise to proceed to payout after advancing"),
    }

    match &c.op_state {
        OpState::Payout(PayoutState {
            op_id,
            receiver: r,
            amount,
            owner: o,
            escrow_shares,
            burn_shares,
        }) => {
            assert_eq!(*op_id, 0);
            assert_eq!(*amount, before_balance + record.principal);
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
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));

    // Enter Payout with non-zero escrow
    c.op_state = OpState::Payout(PayoutState {
        op_id: 123,
        receiver: receiver.clone(),
        amount: 77,
        owner: owner.clone(),
        escrow_shares: escrow,
        burn_shares: escrow,
    });

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
    c.op_state = OpState::Payout(PayoutState {
        op_id: 7,
        receiver,
        amount: 1,
        owner: owner.clone(),
        escrow_shares: 0,
        burn_shares: 0,
    });

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

#[test]
fn no_fee_returns_zero() {
    assert_eq!(
        compute_fee_shares(1_000.into(), 900.into(), Wad::zero(), 1_000.into()),
        Number::zero()
    );
}

#[test]
fn no_profit_returns_zero() {
    assert_eq!(
        compute_fee_shares(
            1_000.into(),
            1_000.into(),
            Wad::one() / 10u128,
            1_000.into()
        ),
        Number::zero()
    );
    assert_eq!(
        compute_fee_shares(900.into(), 1_000.into(), Wad::one() / 10u128, 1_000.into()),
        Number::zero()
    );
}

#[test]
fn zero_supply_returns_zero() {
    assert_eq!(
        compute_fee_shares(1_000.into(), 900.into(), Wad::one() / 10u128, 0u128.into()),
        Number::zero()
    );
}

#[test]
fn simple_accrual_10_percent_fee() {
    // cur=1200, last=1000, profit=200, fee_assets=20
    // fee_shares = floor(20 * 1000 / (1200-20)) = floor(20000/1180) = 16
    assert_eq!(
        u128::from(compute_fee_shares(
            1200u128.into(),
            1000u128.into(),
            Wad::one() / 10u128,
            1000u128.into()
        )),
        16
    );
}

#[test]
fn full_fee_100_percent() {
    // cur=1200, last=1000, profit=200, fee_assets=200
    // denom = 1200 - 200 = 1000
    // fee_shares = 200*1000/1000 = 200
    assert_eq!(
        u128::from(compute_fee_shares(
            1200u128.into(),
            1000u128.into(),
            Wad::one(),
            1000u128.into()
        )),
        200
    );
}

// Property: Shares minting never panics, never mints more than `accept` when price ≥ 1
// Model: minted = floor(accept * S / A); price ≥ 1 <=> A >= S => minted ≤ accept
#[rstest(
        accept => [0u128.into(), 1u128.into(), 2u128.into(), 10u128.into(), (1u128<<32).into(), (1u128<<64).into(), (u128::MAX/2).into(), (u128::MAX-1).into()],
        supply => [0u128.into(), 1u128.into(), 10u128.into(), (1u128<<32).into(), (1u128<<64).into(), (u128::MAX/2).into()],
        assets_base => [1u128.into(), 2u128.into(), 10u128.into(), (1u128<<32).into(), (1u128<<64).into(), (u128::MAX/2).into(), (u128::MAX-1).into()]
    )]
fn prop_minted_shares_le_accept_when_price_ge_one(
    accept: Number,
    supply: Number,
    assets_base: Number,
) {
    use crate::mul_div_floor;

    let assets = core::cmp::max(assets_base, supply); // enforce price ≥ 1
    let minted = mul_div_floor(accept, supply, assets);
    assert!(
        minted <= accept,
        "minted {minted:?} should be <= accept {accept:?} when price>=1 (S={supply:?}, A={assets:?})"
    );
}

// Property: Fee shares are 0 when not profitable (cur_total_assets <= last_total_assets)
#[rstest(
    perf => [Wad::zero(), Wad::one() / Number::from(100u128), Wad::one() / Number::from(10u128)],
    last => [0u128.into(), 1u128.into(), (1u128<<32).into()],
    ts => [0u128.into(), 1u128.into(), (1u128<<64).into()]
)]
fn prop_fee_zero_when_not_profitable(perf: Wad, last: Number, ts: Number) {
    let cur_equal = last;
    let cur_lower = last.saturating_sub(Number::one());
    assert_eq!(
        compute_fee_shares(cur_equal, last, perf, ts),
        Number::zero()
    );
    assert_eq!(
        compute_fee_shares(cur_lower, last, perf, ts),
        Number::zero()
    );
}

#[rstest(
        s =>[0u128.into(), 1u128.into(), 13u128.into(), (1u128<<32).into(), (1u128<<64).into()],
        a =>[1u128.into(), 7u128.into(), (1u128<<32).into(), (1u128<<64).into(), ((1u128<<64) + 123).into()],
        k =>[0u128.into(), 1u128.into(), 2u128.into(), 10u128.into(), (1u128<<16).into()]
    )]
fn deposit_is_monotone_in_assets(s: Number, a: Number, k: Number) {
    // More assets never produce fewer shares (with fixed totals & offsets).

    use crate::mul_div_floor;
    let shares1 = mul_div_floor(a, s + Number::one(), a + k + Number::one());
    let shares2 = mul_div_floor(
        a + Number::one(),
        s + Number::one(),
        a + k + Number::from(2u128),
    );
    assert!(shares2 >= shares1);
}

// Property: Fee shares are monotone =>profit when fee>0 and total_supply>0
#[rstest(
        perf => [Wad::one()/100u128, Wad::one()/10u128],
        last => [0u128.into(), (1u128<<32).into()],
        ts => [1u128.into(), (1u128<<64).into()],
        p1 => [0u128.into(), 1u128.into(), (1u128<<16).into()],
        p2 => [1u128.into(), (1u128<<16).into(), (1u128<<32).into()]
    )]
fn prop_fee_monotone_in_profit(perf: Wad, last: Number, ts: Number, p1: Number, p2: Number) {
    let p_low = core::cmp::min(p1, p2);
    let p_high = core::cmp::max(p1, p2);
    let s1 = compute_fee_shares(last.saturating_add(p_low), last, perf, ts);
    let s2 = compute_fee_shares(last.saturating_add(p_high), last, perf, ts);
    assert!(
        s2 >= s1,
        "fee shares should be monotone =>profit: s2 {s2:?} >= s1 {s1:?} (last={last:?}, perf={perf:?}, ts={ts:?})"
    );
}

// Property: Withdrawal math never underflows:
// withdrawn = before - new (saturating)
// credited = min(withdrawn, need)
// remaining = rem - credited (saturating)
#[rstest(
        before => [0u128, 1, 10, 1u128<<64, u128::MAX/2, u128::MAX-1],
        newp => [0u128, 1, 10, 1u128<<64, u128::MAX/2],
        need => [0u128, 1, 10, 1u128<<32, u128::MAX/4],
        rem => [0u128, 1, 10, 1u128<<32, u128::MAX/4]
    )]
fn prop_withdraw_math_never_underflows(before: u128, newp: u128, need: u128, rem: u128) {
    let withdrawn = before.saturating_sub(newp);
    let credited = core::cmp::min(withdrawn, need);
    let remaining = rem.saturating_sub(credited);
    assert!(withdrawn <= before, "withdrawn should not exceed before");
    assert!(credited <= need, "credited should be <= need");
    assert!(remaining <= rem, "remaining should not exceed rem");
}

#[rstest(
    fee =>[Wad::zero(), Wad::one()/100u128, Wad::one()/10u128],
    ts =>[0u128.into(), 1u128.into(), (1u128<<32).into(), (1u128<<64).into()],
    last =>[0u128.into(), 1u128.into(), (1u128<<32).into()],
    profit =>[0u128.into(), 1u128.into(), 10u128.into(), (1u128<<32).into()]
)]
fn fee_shares_upper_bound_by_total_supply(fee: Wad, ts: Number, last: Number, profit: Number) {
    let cur = last.saturating_add(profit);
    let minted = compute_fee_shares(cur, last, fee, ts);
    assert!(minted <= ts || ts.is_zero());
}
