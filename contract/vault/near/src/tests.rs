#![allow(clippy::pedantic)]

use std::collections::BTreeSet;

use crate::convert::account_id_to_address;
use crate::governance::Gate;
use crate::governance::Timelocks;
use crate::impl_callbacks::reconcile_supply_outcome;
use crate::impl_callbacks::WithdrawReconciliation;
use crate::storage_management::storage_bytes_for_queue_account_id;
use crate::storage_management::yocto_for_bytes;
use crate::test_utils::*;
use crate::Number;
use crate::{Contract, OldContract, StorageKey};
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver as _;
use near_sdk::env;
use near_sdk::serde_json;
use near_sdk::store::IterableMap;
use near_sdk::test_utils::accounts;
use near_sdk::NearToken;
use near_sdk::PromiseOrValue;
use near_sdk::PromiseResult;
use near_sdk::{
    json_types::{U128, U64},
    AccountId,
};
use near_sdk_contract_tools::ft::Nep141 as _;
use near_sdk_contract_tools::ft::Nep141Controller as _;
use near_sdk_contract_tools::mt::Nep245Receiver as _;
use near_sdk_contract_tools::owner::OwnerExternal;
use proptest::prelude::*;
use rstest::{fixture, rstest};
use templar_common::asset::FungibleAsset;
// Import NEAR-specific math types for share math
use templar_common::supply::SupplyPosition;
use templar_common::vault::prelude::{
    compute_fee_shares, compute_fee_shares_from_assets, mul_div_floor, Wad, MAX_MANAGEMENT_FEE_WAD,
    MAX_PERFORMANCE_FEE_WAD,
};
use templar_common::vault::AllocationDelta;
use templar_common::vault::Delta;
use templar_common::vault::DepositMsg;
use templar_common::vault::EscrowSettlement;
use templar_common::vault::Fee;
use templar_common::vault::Fees;
use templar_common::vault::OpState;
use templar_common::vault::PayoutState;
use templar_common::vault::PendingWithdrawal;
use templar_common::vault::{
    AllocatingState, CapGroupId, CapGroupRecord, CapGroupUpdate, CapGroupUpdateKey,
    IdleResyncOutcome, MarketConfiguration, MarketId, RestrictionReason, Restrictions,
    WithdrawingState, MAX_TIMELOCK_NS, YEAR_NS,
};
use templar_vault_kernel::TimestampNs;

#[fixture]
fn vault_id() -> AccountId {
    mk(0)
}

#[fixture]
fn c_vault_env(#[default(vault_id())] vault_id: AccountId) -> Contract {
    setup_env(&vault_id, &vault_id, vec![]);
    new_test_contract(&vault_id)
}

#[fixture]
fn c_owner_env(#[default(vault_id())] vault_id: AccountId) -> Contract {
    build_owner_env(vault_id).contract
}

#[fixture]
fn c_asset_env(#[default(vault_id())] vault_id: AccountId) -> Contract {
    let c = new_test_contract(&vault_id);
    let asset: AccountId = c.underlying_asset.contract_id().into();
    setup_env(&vault_id, &asset, vec![]);
    c
}

struct OwnerEnv {
    vault_id: AccountId,
    owner: AccountId,
    contract: Contract,
}

fn build_owner_env(vault_id: AccountId) -> OwnerEnv {
    let contract = new_test_contract(&vault_id);
    let owner = contract
        .own_get_owner()
        .unwrap_or_else(|| templar_common::panic_with_message("Owner not set"));
    setup_env(&vault_id, &owner, vec![]);
    OwnerEnv {
        vault_id,
        owner,
        contract,
    }
}

fn build_fees(
    performance_fee: Wad,
    management_fee: Wad,
    performance_recipient: AccountId,
    management_recipient: AccountId,
) -> Fees<U128> {
    Fees {
        performance: Fee {
            fee: U128(u128::from(performance_fee)),
            recipient: performance_recipient,
        },
        management: Fee {
            fee: U128(u128::from(management_fee)),
            recipient: management_recipient,
        },
        max_total_assets_growth_rate: None,
    }
}

fn cap_group_record(cap: u128, relative_cap: Wad, principal: u128) -> CapGroupRecord {
    CapGroupRecord {
        cap: templar_curator_primitives::CapGroup::builder()
            .absolute_cap(cap)
            .relative_cap(relative_cap)
            .build(),
        principal,
    }
}

fn cap_group_relative_cap(record: &CapGroupRecord) -> Wad {
    templar_curator_primitives::cap_group_record_relative_cap(record)
}

#[fixture]
fn owner_env(#[default(vault_id())] vault_id: AccountId) -> OwnerEnv {
    build_owner_env(vault_id)
}

#[fixture]
fn enabled_market_100() -> (AccountId, MarketConfiguration) {
    let m = mk(9001);
    let cfg = MarketConfiguration {
        cap: U128(100),
        enabled: true,
        removable_at: TimestampNs::ZERO,
        cap_group_id: None,
    };
    (m, cfg)
}

type MarketFixture = (AccountId, u128, bool, u128, bool);

#[fixture]
fn c(vault_id: AccountId, #[default(Vec::new())] markets: Vec<MarketFixture>) -> Contract {
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    println!("Markets to do {:?}", markets);
    for (market_account, cap, enabled, principal, in_supply_queue) in markets {
        let market_id = c.insert_market_for_tests(
            market_account,
            MarketConfiguration {
                cap: U128(cap),
                enabled,
                removable_at: TimestampNs::ZERO,
                cap_group_id: None,
            },
            principal,
        );

        if in_supply_queue {
            c.supply_queue.push(market_id);
        }
    }

    c
}

fn must_market_id(c: &Contract, market: &AccountId) -> MarketId {
    c.market_id_of(market)
        .unwrap_or_else(|| templar_common::panic_with_message("market missing"))
}

fn must_market_record<'a>(c: &'a Contract, market: &AccountId) -> &'a crate::MarketRecord {
    let market_id = must_market_id(c, market);
    c.markets.get(&market_id).expect("market must exist")
}

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(msg) => *msg,
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(msg) => (*msg).to_string(),
            Err(_) => String::new(),
        },
    }
}

fn supply_msg() -> String {
    serde_json::to_string(&DepositMsg::Supply).unwrap()
}

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
    mk(1)
}

proptest! {
    #[test]
    fn paused_restricts_all_accounts(account in any::<u32>().prop_map(mk)) {
        let r = Restrictions::Paused;
        let mut gate = Gate::new(Some(r));
        gate.paused = true;
        let actor = account;

        let out = std::panic::catch_unwind(|| gate.enforce_policy(actor.as_ref()));
        prop_assert!(out.is_err());
    }

    #[test]
    fn blacklist_restricts_exact_members(
        blacklist in prop::collection::vec(any::<u32>().prop_map(mk), 0..10),
        account in any::<u32>().prop_map(mk)
    ) {
        let set: BTreeSet<AccountId> = blacklist.into_iter().collect();
        let kernel_list = set.iter().map(account_id_to_address).collect();
        let r = Restrictions::Blacklist(kernel_list);
        let actor = account_id_to_address(&account);
        let out = r
            .to_kernel_mode()
            .and_then(|policy| policy.is_restricted(&actor));

        if set.contains(&account) {
            prop_assert_eq!(out, Some(RestrictionReason::Blacklisted));
        } else {
            prop_assert_eq!(out, None);
        }
    }

    #[test]
    fn whitelist_restricts_exact_non_members(
        whitelist in prop::collection::vec(any::<u32>().prop_map(mk), 0..10),
        account in any::<u32>().prop_map(mk)
    ) {
        let set: BTreeSet<AccountId> = whitelist.into_iter().collect();
        let kernel_list = set.iter().map(account_id_to_address).collect();
        let r = Restrictions::Whitelist(kernel_list);
        let actor = account_id_to_address(&account);
        let self_id = account_id_to_address(&mk(0));
        let out = r
            .to_kernel_mode()
            .and_then(|policy| policy.is_restricted_allowing_self(&actor, &self_id));

        if set.contains(&account) || account == mk(0) {
            prop_assert_eq!(out, None);
        } else {
            prop_assert_eq!(out, Some(RestrictionReason::NotWhitelisted));
        }
    }

}

#[test]
fn prop_address_book_rebuilds_to_live_queue_and_op_state() {
    let strategy = (
        prop::collection::vec(
            (
                100u32..200u32,
                200u32..300u32,
                1u128..1_000u128,
                0u64..1_000u64,
            ),
            0..8,
        ),
        prop::option::of((400u32..500u32, 500u32..600u32, 1u128..1_000u128)),
    );

    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 64,
        failure_persistence: None,
        ..ProptestConfig::default()
    });

    let vault_id = mk(10_000);
    let contract = std::cell::RefCell::new(new_test_contract(&vault_id));
    let contract_owner = contract.borrow().own_get_owner().unwrap();

    runner
        .run(&strategy, |(queued, inflight)| {
            setup_env(&vault_id, &contract_owner, vec![]);

            let mut c = contract.borrow_mut();
            c.withdraw_queue = templar_vault_kernel::WithdrawQueue::default();
            c.address_book.clear();
            c.withdraw_route.clear();
            c.set_op_state(OpState::Idle);

            let mut expected = BTreeSet::new();

            for (owner_n, receiver_n, expected_assets, requested_at) in queued {
                let queued_owner = mk(owner_n);
                let queued_receiver = mk(receiver_n);
                let id = c.queue_tail();
                c.insert_pending_withdrawal_for_tests(
                    id,
                    PendingWithdrawal {
                        owner: queued_owner.clone(),
                        receiver: queued_receiver.clone(),
                        escrow_shares: 1,
                        expected_assets,
                        requested_at,
                    },
                );

                expected.insert(account_id_to_address(&queued_owner));
                expected.insert(account_id_to_address(&queued_receiver));
            }

            let stale_owner = mk(900_001);
            let stale_receiver = mk(900_002);
            c.address_book
                .insert(account_id_to_address(&stale_owner), stale_owner.clone());
            c.address_book.insert(
                account_id_to_address(&stale_receiver),
                stale_receiver.clone(),
            );

            if let Some((owner_n, receiver_n, remaining)) = inflight {
                let inflight_owner = mk(owner_n);
                let inflight_receiver = mk(receiver_n);
                let inflight_owner_addr = account_id_to_address(&inflight_owner);
                let inflight_receiver_addr = account_id_to_address(&inflight_receiver);

                c.address_book
                    .insert(inflight_owner_addr, inflight_owner.clone());
                c.address_book
                    .insert(inflight_receiver_addr, inflight_receiver.clone());

                expected.insert(inflight_owner_addr);
                expected.insert(inflight_receiver_addr);

                c.set_op_state(OpState::Withdrawing(WithdrawingState {
                    op_id: 7,
                    request_id: 7,
                    index: 0,
                    remaining,
                    collected: 0,
                    receiver: inflight_receiver_addr,
                    owner: inflight_owner_addr,
                    escrow_shares: 1,
                }));
            } else {
                c.set_op_state(OpState::Idle);
            }

            let actual = c.address_book.keys().copied().collect::<BTreeSet<_>>();
            prop_assert_eq!(actual, expected);
            prop_assert!(!c
                .address_book
                .contains_key(&account_id_to_address(&stale_owner)));
            prop_assert!(!c
                .address_book
                .contains_key(&account_id_to_address(&stale_receiver)));

            Ok(())
        })
        .unwrap_or_else(|e| panic!("property test failed: {e}"));
}

#[test]
fn prop_get_max_deposit_matches_bruteforce() {
    let vault_id = mk(0);
    let contract = std::cell::RefCell::new(new_test_contract(&vault_id));

    let strategy = (
        0u128..=100,
        0u128..=100,
        any::<bool>(),
        0u128..=100,
        0u8..=20,
        0u128..=100,
        0u8..=20,
        0u128..=100,
        0u8..=20,
        prop::collection::vec(
            (
                0u128..=100,
                0u128..=150,
                prop::option::of(0u8..3),
                any::<bool>(),
            ),
            0..=6,
        ),
    );

    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 64,
        failure_persistence: None,
        ..ProptestConfig::default()
    });

    runner
        .run(
            &strategy,
            |(
                idle_total,
                alloc_remaining,
                is_allocating,
                group0_cap,
                group0_rel_tenths,
                group1_cap,
                group1_rel_tenths,
                group2_cap,
                group2_rel_tenths,
                markets,
            )| {
                let mut c = contract.borrow_mut();

                c.markets.clear();
                c.cap_groups.clear();
                c.supply_queue.clear();
                c.op_state = OpState::Idle;
                c.idle_balance = 0;

                let remaining = if is_allocating {
                    alloc_remaining.min(idle_total)
                } else {
                    0
                };
                c.idle_balance = idle_total.saturating_sub(remaining);

                if remaining > 0 {
                    c.op_state = OpState::Allocating(AllocatingState {
                        op_id: 1,
                        index: 0,
                        remaining,
                        plan: vec![],
                    });
                }

                let group_ids = [
                    CapGroupId::try_from("prop-group-0".to_string()).unwrap(),
                    CapGroupId::try_from("prop-group-1".to_string()).unwrap(),
                    CapGroupId::try_from("prop-group-2".to_string()).unwrap(),
                ];

                let mut group_principal = [0u128; 3];
                let mut total_principal = 0u128;
                for (_cap, principal, group_idx, _in_queue) in markets.iter() {
                    total_principal = total_principal.saturating_add(*principal);
                    if let Some(idx) = group_idx {
                        group_principal[*idx as usize] =
                            group_principal[*idx as usize].saturating_add(*principal);
                    }
                }

                let group_caps = [group0_cap, group1_cap, group2_cap];
                let group_rel_tenths = [group0_rel_tenths, group1_rel_tenths, group2_rel_tenths];
                for i in 0..3usize {
                    let relative_cap = Wad::from(Wad::SCALE / 10 * u128::from(group_rel_tenths[i]));
                    c.cap_groups.insert(
                        group_ids[i].clone(),
                        cap_group_record(group_caps[i], relative_cap, group_principal[i]),
                    );
                }

                for (idx, (cap, principal, group_idx, in_queue)) in markets.iter().enumerate() {
                    let market = mk(10_000 + idx as u32);
                    let cap_group_id = group_idx.map(|i| group_ids[i as usize].clone());
                    let cfg = MarketConfiguration {
                        cap: U128(*cap),
                        enabled: true,
                        removable_at: TimestampNs::ZERO,
                        cap_group_id,
                    };

                    let market_id = c.insert_market_for_tests(market, cfg, *principal);

                    if *in_queue && *cap > 0 {
                        c.supply_queue.push(market_id);
                    }
                }

                let base_total_assets = c
                    .idle_balance
                    .saturating_add(total_principal)
                    .saturating_add(remaining);
                prop_assert_eq!(c.total_assets_for_caps(), base_total_assets);

                let rounding_slack = c.relative_cap_rounding_slack();
                let markets = c.supply_queue_market_infos();
                let upper = markets
                    .iter()
                    .fold(0u128, |acc, market| acc.saturating_add(market.cap_room));

                let mut expected = 0u128;
                for x in 0..=upper {
                    let total_assets = base_total_assets.saturating_add(x);
                    let room = c.max_allocatable_room_at_precomputed(total_assets, &markets);
                    if x <= room.saturating_add(rounding_slack) {
                        expected = x;
                    }
                }

                prop_assert_eq!(c.get_max_deposit().0, expected);
                Ok(())
            },
        )
        .unwrap();
}

#[rstest(len => [2usize, 3, 5])]
#[should_panic = "Duplicate market"]
fn prop_supply_queue_mustnt_have_duplicates(len: usize) {
    let mut c = new_test_contract(&mk(0));
    setup_env(&mk(0), &mk(1), vec![]);

    // Build a queue with a duplicate market id
    let base = 100u32;
    let dup = MarketId::from(base);
    let mut queue: Vec<MarketId> = Vec::with_capacity(len);
    if len >= 1 {
        queue.push(dup);
    }
    for i in 1..len.saturating_sub(1) {
        queue.push(MarketId::from(base + i as u32));
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
    let user = mk(1);
    c.deposit_unchecked(&user, 1_000)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));
    c.idle_balance = 1_000;

    c.fees.performance.fee = Wad::one() / 10;

    // Baseline: anchor = current, so no profit => no fee
    c.fee_anchor.total_assets = c.get_total_assets();
    c.fee_anchor.timestamp_ns = env::block_timestamp().into();
    let ts_before = c.total_supply();
    c.internal_accrue_fee();
    assert_eq!(c.total_supply(), ts_before, "no profit => no fee minted");

    // Simulate profit: increase idle_balance; now fees should mint
    c.idle_balance = 1_500;
    let expect = compute_fee_shares(
        c.get_total_assets().0.into(),
        c.fee_anchor.total_assets.0.into(),
        c.fees.performance.fee,
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
    let owner = mk(1);
    let queued_receiver = mk(9);

    c.deposit_unchecked(&near_sdk::env::current_account_id(), 100)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    c.idle_balance = 1_000;

    // Partial payout scenario: collected/requested = 200/500 => burn 40% of escrowed shares
    let amount = 200;
    let op_id = 1;
    c.insert_pending_withdrawal_for_tests(
        0,
        PendingWithdrawal {
            receiver: queued_receiver.clone(),
            owner: owner.clone(),
            escrow_shares: 100,
            expected_assets: amount,
            requested_at: 0,
        },
    );
    c.remember_account_mapping(account_id_to_address(&owner), owner.clone());
    c.remember_account_mapping(account_id_to_address(&receiver), receiver.clone());
    c.remember_account_mapping(
        account_id_to_address(&queued_receiver),
        queued_receiver.clone(),
    );
    c.address_book
        .insert(account_id_to_address(&owner), owner.clone());
    c.address_book
        .insert(account_id_to_address(&receiver), receiver.clone());
    c.address_book.insert(
        account_id_to_address(&queued_receiver),
        queued_receiver.clone(),
    );
    c.set_op_state(OpState::Payout(PayoutState {
        op_id,
        request_id: op_id,
        receiver: account_id_to_address(&receiver),
        amount,
        owner: account_id_to_address(&owner),
        escrow_shares: 100,
        burn_shares: 40,
    }));

    let supply_before = c.total_supply();
    c.payment_01_reconcile_idle_or_refund(Ok(()), op_id, receiver, U128(amount));

    // Idle decreased by payout before payout is initiated
    // Only burn_shares are burned from total supply
    assert_eq!(c.total_supply(), supply_before - 40);
    assert!(matches!(c.op_state, OpState::Idle));
}

#[test]
#[should_panic = "unauthorized market"]
fn set_supply_queue_rejects_zero_cap() {
    let mut c = new_test_contract(&mk(0));
    setup_env(&mk(0), &mk(1), vec![]);

    let market_id = c.insert_market_for_tests(mk(100), MarketConfiguration::default(), 0);
    c.set_supply_queue(vec![market_id]);
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

    let sender = mk(1);
    let wrong_token: AccountId = "wrong.token".parse().unwrap();
    let deposit = 1_000u128;

    let _ = c.execute_supply(sender.clone(), wrong_token.clone(), deposit);
}

#[rstest]
fn start_allocation_reserves_only_amount(
    #[with(vault_id(), vec![(mk(2000), 80, true, 0, true)])] mut c: Contract,
) {
    // Idle = 100, so max_room (80) should clamp allocation
    c.idle_balance = 100;
    assert_eq!(c.get_max_deposit().0, 80, "sanity: max room must be 80");

    // Reserve only the amount to allocate (intended behavior)
    let total = c.get_max_deposit().0.min(c.idle_balance);
    owner_call_env(env::current_account_id(), &owner());
    let m1 = mk(2000);
    let market_id = c
        .market_id_of(&m1)
        .unwrap_or_else(|| templar_common::panic_with_message("market missing"));
    c.allocate(AllocationDelta::Supply(Delta::new(market_id, total)))
        .detach();

    let rec = c
        .markets
        .get_mut(&market_id)
        .unwrap_or_else(|| templar_common::panic_with_message("market missing"));
    rec.principal = 80;
    // Force completion and exit op
    match c.op_state.clone() {
        crate::OpState::Allocating(AllocatingState {
            op_id, index, plan, ..
        }) => {
            c.op_state = crate::OpState::Allocating(AllocatingState {
                op_id,
                index,
                remaining: 0,
                plan,
            });
        }
        s => {
            panic!("expected Allocating state, got {:?}", s);
        }
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

#[rstest]
#[should_panic = "Insufficient principal"]
fn allocate_withdraw_insufficient_principal_panics(owner_env: OwnerEnv) {
    let OwnerEnv { mut contract, .. } = owner_env;

    let market_id = contract.insert_market_for_tests(mk(9201), MarketConfiguration::default(), 0);

    let _ = contract.allocate(AllocationDelta::Withdraw(Delta::new(market_id, 1)));
}

#[rstest]
#[should_panic = "Insufficient principal"]
fn allocate_withdraw_zero_amount_panics(owner_env: OwnerEnv) {
    let OwnerEnv { mut contract, .. } = owner_env;

    let market_id = contract.insert_market_for_tests(mk(9202), MarketConfiguration::default(), 0);

    let _ = contract.allocate(AllocationDelta::Withdraw(Delta::new(market_id, 100)));
}

#[rstest]
fn allocate_withdraw_returns_promise_and_does_not_mutate(owner_env: OwnerEnv) {
    let OwnerEnv { mut contract, .. } = owner_env;

    let market_id = contract.insert_market_for_tests(mk(9203), MarketConfiguration::default(), 40);

    let principal_before = contract.principal_of(market_id);
    assert_eq!(principal_before, 40, "sanity: principal set");

    let res = contract.allocate(AllocationDelta::Withdraw(Delta::new(market_id, 100)));
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise for withdraw allocation"),
    }

    assert!(
        matches!(contract.op_state, OpState::Idle),
        "allocate withdraw should not change op_state"
    );
    assert_eq!(
        contract.principal_of(market_id),
        principal_before,
        "principal must not change when only creating a withdraw request"
    );
}

#[test]
fn allocate_accrues_pending_fee_shares() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    c.deposit_unchecked(&owner, 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    c.fees.performance.fee = Wad::one() / 10;
    c.idle_balance += 500;

    let market_account = mk(9204);
    let cfg = MarketConfiguration {
        cap: U128(10_000),
        enabled: true,
        removable_at: TimestampNs::ZERO,
        cap_group_id: None,
    };
    let market_id = c.insert_market_for_tests(market_account, cfg, 0);
    c.supply_queue.push(market_id);

    let fee_recipient = c.fees.performance.recipient.clone();
    let balance_before = c.balance_of(&fee_recipient);
    let assets_before = c.get_total_assets().0;

    owner_call_env(vault_id.clone(), &owner);
    let res = c.allocate(AllocationDelta::Supply(Delta::new(market_id, 50)));
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise for supply allocation"),
    }

    let balance_after = c.balance_of(&fee_recipient);
    assert!(
        balance_after > balance_before,
        "allocate should mint fee shares when profit exists before planning allocation",
    );
    assert_eq!(
        c.get_last_total_assets().0,
        assets_before,
        "accrual should snapshot assets before allocation bookkeeping",
    );
}

#[test]
fn sentinel_can_only_allocate_withdrawals() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let sentinel = c.get_configuration().sentinel;
    setup_env(&vault_id, &sentinel, vec![]);

    let market_id = c.insert_market_for_tests(mk(9300), MarketConfiguration::default(), 50);

    let withdraw_res = c.allocate(AllocationDelta::Withdraw(Delta::new(market_id, 10)));
    assert!(
        matches!(withdraw_res, PromiseOrValue::Promise(_)),
        "Sentinel withdraw allocation should return a Promise"
    );

    let supply_attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        c.allocate(AllocationDelta::Supply(Delta::new(market_id, 10)))
    }));
    assert!(
        supply_attempt.is_err(),
        "Sentinel must not be allowed to run supply allocations"
    );
}

#[test]
fn sentinel_can_execute_rebalance_withdrawal() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let sentinel = c.get_configuration().sentinel;
    setup_env(&vault_id, &sentinel, vec![]);

    let market_id = c.insert_market_for_tests(mk(9400), MarketConfiguration::default(), 25);

    let res = c.execute_rebalance_withdrawal(market_id, None);
    assert!(
        matches!(res, PromiseOrValue::Promise(_)),
        "Sentinel rebalance should return a Promise"
    );
}

#[rstest(
    escrow, collected, requested, expect,
    case(100u128, 200u128, 500u128, 40u128),  // 40%
    case(123u128, 0u128, 456u128, 0u128),     // no collection => no burn
    case(100u128, 1u128, 3u128, 34u128),      // ceil on rounding
    case(50u128, 10u128, 0u128, 50u128)       // zero request => full burn
)]
fn compute_burn_shares_cases(escrow: u128, collected: u128, requested: u128, expect: u128) {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let burn = templar_vault_kernel::compute_idle_settlement(escrow, requested, collected)
        .map_or(0, |result| result.settlement.to_burn);

    assert_eq!(
        burn, expect,
        "kernel idle settlement should drive proportional payout burn"
    );
}

#[test]
fn compute_effective_totals_fee_share_and_virtuals() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let cur = 1_500u128.into();
    let last = 1_000u128.into();
    let perf = Wad::one() / 10;
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
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let s1: (u128, u128) = EscrowSettlement::new(100, 40).into();
    assert_eq!(s1, (40u128, 60u128));

    let s2: (u128, u128) = EscrowSettlement::new(100, 200).into();
    assert_eq!(s2, (100u128, 0u128));

    let s3: (u128, u128) = EscrowSettlement::new(0, 50).into();
    assert_eq!(s3, (0u128, 0u128));
}

#[rstest]
fn cap_zero_keeps_enabled_and_submit_removal_works(owner_env: OwnerEnv) {
    let OwnerEnv {
        vault_id,
        owner,
        mut contract,
    } = owner_env;

    let m = mk(8001);

    // Seed a known, enabled market with cap > 0
    let cfg = MarketConfiguration {
        cap: U128(10_000),
        enabled: true,
        removable_at: TimestampNs::ZERO,
        cap_group_id: None,
    };
    let _ = contract.insert_market_for_tests(m.clone(), cfg, 0);

    contract.submit_cap(m.clone(), U128(0));
    let cfg_after = &must_market_record(&contract, &m).cfg;
    assert_eq!(cfg_after.cap.0, 0, "cap must be updated to 0");
    assert!(cfg_after.enabled, "enabled must remain true when cap is 0");

    set_block_ts(&vault_id, &owner, 2);

    contract.submit_market_removal(m.clone());
    let cfg_after2 = must_market_record(&contract, &m);
    assert!(
        cfg_after2.cfg.removable_at > TimestampNs::ZERO,
        "removal must be scheduled"
    );
}

#[rstest]
fn accept_cap_raise_enables_and_cap_zero_keeps_enabled(owner_env: OwnerEnv) {
    let OwnerEnv {
        vault_id,
        owner,
        mut contract,
    } = owner_env;

    let m = mk(8002);

    let _ = contract.insert_market_for_tests(m.clone(), MarketConfiguration::default(), 0);

    // Submit raise -> pending
    let raise = 5u128;
    set_ctx(&vault_id, &owner, None, Some(yocto_for_bytes(10_000)));
    contract.submit_cap(m.clone(), U128(raise));

    set_ctx(
        &vault_id,
        &owner,
        Some(env::block_timestamp() + 1_000_000_000),
        None,
    );
    contract.accept_cap(m.clone());

    let cfg1 = &must_market_record(&contract, &m).cfg;
    assert_eq!(cfg1.cap.0, raise);
    assert!(cfg1.enabled, "market should be enabled after raise");

    // Now lower back to 0 and ensure enabled stays true
    contract.submit_cap(m.clone(), U128(0));
    let cfg2 = &must_market_record(&contract, &m).cfg;
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

#[test]
fn withdraw_reconcile_uses_creditable_when_principal_exceeds_inflow() {
    // before_principal drops by 60, but only 50 tokens actually arrive.
    let before_principal = 100u128;
    let reported_principal = 40u128; // principal_delta = 60
    let before_balance = U128(1_000);

    let (principal_delta, inflow, creditable) = Contract::compute_withdraw_deltas(
        U128(before_principal),
        U128(reported_principal),
        U128(1_050),
        before_balance,
    );

    assert_eq!(principal_delta, 60);
    assert_eq!(inflow, 50);
    assert_eq!(creditable, 50, "creditable must be min(delta, inflow)");

    let effective_principal = before_principal.saturating_sub(creditable);
    let need = 100u128;
    let rem = need;
    let coll = 0u128;

    let res = crate::impl_callbacks::reconcile_withdraw_outcome(
        before_principal,
        effective_principal,
        rem,
        coll,
    );

    // We should only ever credit up to the inflow/creditable amount.
    assert_eq!(res.payout_delta, creditable);
    assert!(res.payout_delta <= inflow);
    assert_eq!(res.remaining_next, rem.saturating_sub(creditable));
    assert_eq!(res.collected_next, coll.saturating_add(creditable));
    assert_eq!(res.idle_delta, creditable);
}

#[test]
fn withdraw_under_credit_emits_inflow_mismatch_and_clamps() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let before_principal = 1_000u128;
    let market_id =
        c.insert_market_for_tests(mk(8008), MarketConfiguration::default(), before_principal);
    c.withdraw_route = vec![market_id].into();

    let need = 150u128;
    let collected_start = 0u128;

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 1,
        request_id: 1,
        index: 0,
        remaining: need,
        receiver: account_id_to_address(&mk(9)),
        collected: collected_start,
        owner: account_id_to_address(&mk(1)),
        escrow_shares: 100,
    });
    c.remember_account_mapping(account_id_to_address(&mk(1)), mk(1));
    c.remember_account_mapping(account_id_to_address(&mk(9)), mk(9));

    let _before_idle = c.idle_balance;

    let before_balance = U128(1_000_000);
    let after_balance = Ok(U128(1_000_100)); // inflow = 100
    let after_balance_val = after_balance.as_ref().unwrap().0;
    let _inflow = after_balance_val.saturating_sub(before_balance.0);
    let reported_principal = U128(100); // principal_delta = 900 >> inflow

    let res = c.execute_withdraw_03_settle(
        after_balance,
        1,
        market_id,
        U64(0),
        U128(before_principal),
        reported_principal,
        before_balance,
    );
    match res {
        PromiseOrValue::Value(()) | PromiseOrValue::Promise(_) => {}
    }

    let logs = near_sdk::test_utils::get_logs();
    let joined = logs.join("\n");
    assert!(
        joined.contains("\"event\":\"withdrawal_accounting\"")
            && joined.contains("\"kind\":\"InflowMismatch\""),
        "expected withdrawal_accounting InflowMismatch event, got logs: {joined:?}",
    );

    if let OpState::Withdrawing(WithdrawingState {
        remaining,
        collected,
        ..
    }) = c.op_state
    {
        let requested = need + collected_start;
        assert!(
            collected <= requested,
            "collected must not exceed requested total"
        );
        assert_eq!(
            remaining.saturating_add(collected),
            requested,
            "remaining + collected must stay constant",
        );
    } else {
        panic!("expected Withdrawing state after under-credit scenario");
    }

    let rec = c.markets.get(&market_id).expect("market must exist");
    assert!(
        rec.principal <= before_principal,
        "principal must not increase during withdraw settlement",
    );

    assert_eq!(
        c.idle_balance, after_balance_val,
        "idle balance should resync to actual balance",
    );
}

#[test]
fn withdraw_over_credit_emits_overpay_and_clamps_to_requested() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let before_principal = 1_000u128;
    let market_id =
        c.insert_market_for_tests(mk(8009), MarketConfiguration::default(), before_principal);
    c.withdraw_route = vec![market_id].into();

    let need = 120u128;

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 2,
        request_id: 2,
        index: 0,
        remaining: need,
        receiver: account_id_to_address(&mk(10)),
        collected: 0,
        owner: account_id_to_address(&mk(2)),
        escrow_shares: 100,
    });
    c.remember_account_mapping(account_id_to_address(&mk(2)), mk(2));
    c.remember_account_mapping(account_id_to_address(&mk(10)), mk(10));

    let before_balance = U128(1_000_000);
    let after_balance = Ok(U128(1_000_200)); // inflow = 200
    let after_balance_val = after_balance.as_ref().unwrap().0;
    let reported_principal = U128(850); // principal_delta = 150

    let res = c.execute_withdraw_03_settle(
        after_balance,
        2,
        market_id,
        U64(0),
        U128(before_principal),
        reported_principal,
        before_balance,
    );
    match res {
        PromiseOrValue::Value(()) | PromiseOrValue::Promise(_) => {}
    }

    let logs = near_sdk::test_utils::get_logs();
    let joined = logs.join("\n");
    assert!(
        joined.contains("\"event\":\"withdrawal_accounting\"")
            && joined.contains("\"kind\":\"OverpayCredited\""),
        "expected withdrawal_accounting OverpayCredited event, got logs: {joined:?}",
    );

    if let OpState::Payout(PayoutState { amount, .. }) = c.op_state {
        assert_eq!(amount, need, "payout amount must be capped at requested");
        assert_eq!(
            c.idle_balance,
            after_balance_val.saturating_sub(need),
            "idle should reflect actual balance minus payout",
        );
    } else {
        panic!("expected Payout state after over-credit scenario");
    }
}

#[test]
fn withdraw_idle_balance_resyncs_on_external_deposit() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let before_principal = 1_000u128;
    let market_id =
        c.insert_market_for_tests(mk(8010), MarketConfiguration::default(), before_principal);
    c.withdraw_route = vec![market_id].into();

    c.idle_balance = 1_150; // simulate deposit arriving after before_balance snapshot

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 42,
        request_id: 42,
        index: 0,
        remaining: 300,
        receiver: account_id_to_address(&mk(11)),
        collected: 0,
        owner: account_id_to_address(&mk(3)),
        escrow_shares: 200,
    });
    c.remember_account_mapping(account_id_to_address(&mk(3)), mk(3));
    c.remember_account_mapping(account_id_to_address(&mk(11)), mk(11));

    let before_balance = U128(1_000);
    let after_balance = Ok(U128(1_150));
    let reported_principal = U128(before_principal);

    let res = c.execute_withdraw_03_settle(
        after_balance,
        42,
        market_id,
        U64(0),
        U128(before_principal),
        reported_principal,
        before_balance,
    );
    match res {
        PromiseOrValue::Value(()) | PromiseOrValue::Promise(_) => {}
    }

    assert_eq!(c.idle_balance, 1_150, "idle must resync to actual balance");

    match &c.op_state {
        OpState::Withdrawing(WithdrawingState {
            remaining,
            collected,
            index,
            ..
        }) => {
            assert_eq!(*index, 0);
            assert_eq!(*remaining, 150, "extra inflow should reduce remaining");
            assert_eq!(*collected, 150, "extra inflow should increase collected");
        }
        other => panic!("expected Withdrawing state after settlement, got {other:?}"),
    }
}

#[test]
fn withdraw_over_credit_triggers_payout_with_capped_amount() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let market = mk(8011);
    let before_principal = 1_000u128;
    let market_id = c.insert_market_for_tests(
        market.clone(),
        MarketConfiguration::default(),
        before_principal,
    );
    c.withdraw_route = vec![market_id].into();

    let need = 120u128;
    let inflow = 200u128;

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 43,
        request_id: 43,
        index: 0,
        remaining: need,
        receiver: account_id_to_address(&mk(12)),
        collected: 0,
        owner: account_id_to_address(&mk(4)),
        escrow_shares: 150,
    });
    c.remember_account_mapping(account_id_to_address(&mk(4)), mk(4));
    c.remember_account_mapping(account_id_to_address(&mk(12)), mk(12));

    let before_balance = U128(1_000_000);
    let after_balance = Ok(U128(1_000_000 + inflow));
    let after_balance_val = after_balance.as_ref().unwrap().0;
    let reported_principal = U128(before_principal);

    let res = c.execute_withdraw_03_settle(
        after_balance,
        43,
        market_id,
        U64(0),
        U128(before_principal),
        reported_principal,
        before_balance,
    );
    match res {
        PromiseOrValue::Value(()) | PromiseOrValue::Promise(_) => {}
    }

    let logs = near_sdk::test_utils::get_logs();
    let joined = logs.join("\n");
    assert!(
        joined.contains("\"event\":\"withdrawal_accounting\"")
            && joined.contains("\"kind\":\"OverpayCredited\""),
        "expected withdrawal_accounting OverpayCredited event, got logs: {joined:?}",
    );

    if let OpState::Payout(PayoutState { amount, .. }) = c.op_state {
        assert_eq!(amount, need, "payout amount must be clamped to requested",);
    } else {
        panic!("expected Payout state after over-credit scenario");
    }

    let rec = c.markets.get(&market_id).expect("market must exist");
    assert!(
        rec.principal <= before_principal,
        "principal must not increase during withdraw settlement",
    );

    let expected_idle = after_balance_val.saturating_sub(need);
    assert_eq!(
        c.idle_balance, expected_idle,
        "idle balance should reflect actual balance minus payout",
    );
}

#[test]
fn withdraw_balance_read_failure_stops_operation() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let market = mk(8012);
    let before_principal = 500u128;
    let market_id = c.insert_market_for_tests(
        market.clone(),
        MarketConfiguration::default(),
        before_principal,
    );
    c.withdraw_route = vec![market_id].into();

    let owner = mk(5);
    let receiver = mk(13);
    c.deposit_unchecked(&near_sdk::env::current_account_id(), 100)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    c.insert_pending_withdrawal_for_tests(
        0,
        PendingWithdrawal {
            owner: owner.clone(),
            receiver: receiver.clone(),
            escrow_shares: 100,
            expected_assets: 200,
            requested_at: 0,
        },
    );
    c.withdraw_queue.next_withdraw_to_execute = 0;

    let op_id = 9;
    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id,
        request_id: 0,
        index: 0,
        remaining: 200,
        receiver: account_id_to_address(&receiver),
        collected: 0,
        owner: account_id_to_address(&owner),
        escrow_shares: 100,
    });

    let res = c.execute_withdraw_03_settle(
        Err(near_sdk::PromiseError::Failed),
        op_id,
        market_id,
        U64(0),
        U128(before_principal),
        U128(before_principal),
        U128(1_000),
    );

    assert!(matches!(res, PromiseOrValue::Value(())));
    assert!(matches!(c.op_state, OpState::Idle));
    assert!(c.withdraw_route.is_empty(), "route must be cleared on stop");
    assert_eq!(
        c.pending_withdrawals_len(),
        0,
        "pending withdrawal should be dequeued"
    );
}

#[test]
fn rebalance_resyncs_idle_on_external_deposit() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let market = mk(8012);
    let before_principal = 500u128;
    let market_id = c.insert_market_for_tests(
        market.clone(),
        MarketConfiguration::default(),
        before_principal,
    );

    c.idle_balance = 1_150;
    c.op_state = OpState::Allocating(AllocatingState {
        op_id: 7,
        index: 0,
        remaining: 0,
        plan: vec![],
    });

    let before_balance = U128(1_000);
    let after_balance = Ok(U128(1_150));

    let res = c.rebalance_withdraw_03_settle(
        after_balance,
        7,
        market_id,
        U64(0),
        U128(before_principal),
        U128(before_principal),
        before_balance,
    );
    match res {
        PromiseOrValue::Value(()) | PromiseOrValue::Promise(_) => {}
    }

    assert_eq!(
        c.idle_balance, 1_150,
        "rebalance must resync to actual balance"
    );
    assert!(matches!(c.op_state, OpState::Idle));

    let rec = c.markets.get(&market_id).expect("market must exist");
    assert_eq!(rec.principal, before_principal);
}

#[test]
fn rebalance_balance_read_failure_stops_operation() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let market = mk(8014);
    let before_principal = 600u128;
    let market_id = c.insert_market_for_tests(
        market.clone(),
        MarketConfiguration::default(),
        before_principal,
    );

    let op_id = 11;

    let lease = c.market_execution_lock.lock(market_id, op_id, u64::MAX / 2);
    c.op_state = OpState::Allocating(AllocatingState {
        op_id,
        index: 0,
        remaining: 0,
        plan: vec![],
    });

    let res = c.rebalance_withdraw_03_settle(
        Err(near_sdk::PromiseError::Failed),
        op_id,
        market_id,
        U64(lease.fencing_token.0),
        U128(before_principal),
        U128(before_principal),
        U128(1_000),
    );

    assert!(matches!(res, PromiseOrValue::Value(())));
    assert!(matches!(c.op_state, OpState::Idle));
    assert!(
        !c.has_pending_market_withdrawal(),
        "locks should be cleared on stop"
    );

    let logs = near_sdk::test_utils::get_logs();
    let joined = logs.join("\n");
    assert!(
        joined.contains("\"event\":\"rebalance_withdraw_stopped\"")
            && joined.contains("balance read failed"),
        "expected rebalance stop event with balance read failed reason"
    );
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
    let vault_id = mk(0);
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

#[test]
fn refresh_markets_updates_principals_and_emits_events() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);

    let m1 = mk(7001);
    let m2 = mk(7002);
    let m1_id = c.insert_market_for_tests(m1, MarketConfiguration::default(), 10);
    let m2_id = c.insert_market_for_tests(m2, MarketConfiguration::default(), 20);

    set_ctx_with_gas(
        &vault_id,
        &vault_id,
        Some(crate::DEFAULT_REFRESH_COOLDOWN_NS.saturating_add(1)),
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );

    let op_id = c.next_op_id;
    let _ = c.refresh_markets(vec![]);
    assert!(matches!(c.op_state, OpState::Refreshing(_)));
    assert_eq!(c.last_refresh_ns, 0);

    let pos1 = SupplyPosition::new(0);
    let _ = c.refresh_01_settle(Ok(Some(pos1)), m1_id, op_id, 0, U128(10));
    assert_eq!(c.last_refresh_ns, 0);

    let pos2 = SupplyPosition::new(0);
    let res = c.refresh_01_settle(Ok(Some(pos2)), m2_id, op_id, 1, U128(20));
    if let PromiseOrValue::Value(report) = res {
        assert_eq!(report.total_assets, c.get_total_assets());
        assert_eq!(c.last_refresh_ns, u64::from(report.refreshed_at));
    }
    assert!(matches!(c.op_state, OpState::Idle));

    let logs = near_sdk::test_utils::get_logs().join("\n");
    assert!(
        logs.contains("refresh_started"),
        "missing refresh start event logs"
    );
    assert!(
        logs.contains("refresh_completed"),
        "missing refresh completed event logs"
    );
}

#[test]
fn refresh_markets_uses_deposit_principal_not_unharvested_yield() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);

    let market = mk(7010);
    let market_id = c.insert_market_for_tests(market, MarketConfiguration::default(), 10);

    set_ctx_with_gas(
        &vault_id,
        &vault_id,
        Some(crate::DEFAULT_REFRESH_COOLDOWN_NS.saturating_add(1)),
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );

    let op_id = c.next_op_id;
    let _ = c.refresh_markets(vec![market_id]);
    assert!(matches!(c.op_state, OpState::Refreshing(_)));

    let mut pos = SupplyPosition::new(0);
    pos.borrow_asset_yield.add_once(250u128.into());
    let _ = c.refresh_01_settle(Ok(Some(pos)), market_id, op_id, 0, U128(10));

    assert!(matches!(c.op_state, OpState::Idle));
    assert_eq!(c.principal_of(market_id), 0);
    assert_eq!(c.get_total_assets().0, c.idle_balance);
}

#[test]
fn stale_principal_before_refresh_underprices_new_deposits() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);

    let owner = mk(10);
    c.deposit_unchecked(&owner, 100)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));

    c.idle_balance = 0;
    let market = mk(7011);
    let market_id = c.insert_market_for_tests(market, MarketConfiguration::default(), 100);
    c.fees.performance.fee = Wad::zero();
    c.fees.management.fee = Wad::zero();
    c.fee_anchor.total_assets = U128(c.get_total_assets().0);
    c.fee_anchor.timestamp_ns = env::block_timestamp().into();

    let deposit_assets = U128(50);
    let minted_before_refresh = c.preview_deposit(deposit_assets).0;
    assert!(minted_before_refresh > 0);

    c.set_market_principal(market_id, 150);

    let assets_after_refresh = c.convert_to_assets(U128(minted_before_refresh)).0;
    assert!(assets_after_refresh > deposit_assets.0);
}

#[test]
#[should_panic(expected = "Refresh throttled")]
fn refresh_markets_throttles_without_time_advance() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);

    set_ctx_with_gas(
        &vault_id,
        &vault_id,
        None,
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );

    let market_id = c.insert_market_for_tests(mk(7003), MarketConfiguration::default(), 0);

    set_ctx_with_gas(
        &vault_id,
        &vault_id,
        Some(crate::DEFAULT_REFRESH_COOLDOWN_NS.saturating_add(1)),
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );

    let op_id = c.next_op_id;
    let _ = c.refresh_markets(vec![market_id]);

    let pos = SupplyPosition::new(0);
    let _ = c.refresh_01_settle(Ok(Some(pos)), market_id, op_id, 0, U128(0));
    assert!(matches!(c.op_state, OpState::Idle));

    set_ctx_with_gas(
        &vault_id,
        &vault_id,
        None,
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );
    c.refresh_markets(vec![market_id]).detach();
}

#[test]
fn refresh_markets_does_not_consume_cooldown_before_completion() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);

    set_ctx_with_gas(
        &vault_id,
        &vault_id,
        None,
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );

    let market_id = c.insert_market_for_tests(mk(7004), MarketConfiguration::default(), 0);

    set_ctx_with_gas(
        &vault_id,
        &vault_id,
        Some(crate::DEFAULT_REFRESH_COOLDOWN_NS.saturating_add(1)),
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );

    let op_id = c.next_op_id;
    let _ = c.refresh_markets(vec![market_id]);
    assert_eq!(c.last_refresh_ns, 0);

    let ignored = c.refresh_01_settle(
        Ok(Some(SupplyPosition::new(0))),
        market_id,
        op_id,
        99,
        U128(0),
    );
    assert!(matches!(ignored, PromiseOrValue::Value(_)));
    assert!(matches!(c.op_state, OpState::Idle));
    assert_eq!(c.last_refresh_ns, 0);

    set_ctx_with_gas(
        &vault_id,
        &vault_id,
        Some(crate::DEFAULT_REFRESH_COOLDOWN_NS.saturating_add(1)),
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );

    let _ = c.refresh_markets(vec![market_id]);
    assert!(matches!(c.op_state, OpState::Refreshing(_)));
}

#[test]
fn refresh_markets_with_no_targets_stamps_completion_timestamp() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);

    set_ctx_with_gas(
        &vault_id,
        &vault_id,
        Some(crate::DEFAULT_REFRESH_COOLDOWN_NS.saturating_add(1)),
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );

    let result = c.refresh_markets(vec![]);

    let PromiseOrValue::Value(report) = result else {
        panic!("expected immediate refresh completion");
    };

    assert!(matches!(c.op_state, OpState::Idle));
    assert_eq!(c.last_refresh_ns, u64::from(report.refreshed_at));
}

#[test]
fn idle_resync_callback_increase_updates_idle_and_bumps_fee_anchor() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    c.idle_resync_last_ns = 0;
    c.idle_resync_cooldown_ns = 120_000_000_000;
    c.idle_resync_inflight_op_id = 0;

    let caller = mk(1);

    let op_id = 42u64;
    c.idle_resync_inflight_op_id = op_id;
    c.op_state = OpState::Allocating(AllocatingState {
        op_id,
        index: 0,
        remaining: 0,
        plan: vec![],
    });

    c.idle_balance = 100;
    c.fee_anchor.total_assets = 1_000.into();
    c.fee_anchor.timestamp_ns = 777.into();

    let finished_at_ns = 1_234u64;
    set_ctx_with_gas(&vault_id, &caller, Some(finished_at_ns), None, None);

    let report =
        c.resync_idle_balance_01_settle(Ok(U128(150)), op_id, caller.clone(), U128(100), 0);

    assert_eq!(report.outcome, IdleResyncOutcome::Ok);
    assert_eq!(report.before_idle, U128(100));
    assert_eq!(report.actual_idle, U128(150));
    assert_eq!(report.after_idle, U128(150));
    assert_eq!(report.increased_by, U128(50));
    assert_eq!(report.decreased_by, U128(0));
    assert_eq!(report.fee_anchor_bump, U128(50));
    assert_eq!(report.resynced_at_ns.0, finished_at_ns);

    assert_eq!(c.idle_balance, 150);
    assert_eq!(c.fee_anchor.total_assets.0, 1_050);
    assert_eq!(c.fee_anchor.timestamp_ns.0, 777);
    assert!(matches!(c.op_state, OpState::Idle));
    assert_eq!(c.idle_resync_inflight_op_id, 0);

    let joined = near_sdk::test_utils::get_logs().join("\n");
    assert!(joined.contains("\"event\":\"idle_resync_completed\""));
}

#[test]
fn idle_resync_callback_decrease_updates_idle_without_bumping_fee_anchor() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    c.idle_resync_last_ns = 0;
    c.idle_resync_cooldown_ns = 120_000_000_000;
    c.idle_resync_inflight_op_id = 0;

    let caller = mk(1);

    let op_id = 42u64;
    c.idle_resync_inflight_op_id = op_id;
    c.op_state = OpState::Allocating(AllocatingState {
        op_id,
        index: 0,
        remaining: 0,
        plan: vec![],
    });

    c.idle_balance = 100;
    c.fee_anchor.total_assets = 1_000.into();
    c.fee_anchor.timestamp_ns = 777.into();

    let finished_at_ns = 1_234u64;
    set_ctx_with_gas(&vault_id, &caller, Some(finished_at_ns), None, None);

    let report = c.resync_idle_balance_01_settle(Ok(U128(80)), op_id, caller.clone(), U128(100), 0);

    assert_eq!(report.outcome, IdleResyncOutcome::Ok);
    assert_eq!(report.before_idle, U128(100));
    assert_eq!(report.actual_idle, U128(80));
    assert_eq!(report.after_idle, U128(80));
    assert_eq!(report.increased_by, U128(0));
    assert_eq!(report.decreased_by, U128(20));
    assert_eq!(report.fee_anchor_bump, U128(0));
    assert_eq!(report.resynced_at_ns.0, finished_at_ns);

    assert_eq!(c.idle_balance, 80);
    assert_eq!(c.fee_anchor.total_assets.0, 1_000);
    assert_eq!(c.fee_anchor.timestamp_ns.0, 777);
    assert!(matches!(c.op_state, OpState::Idle));
    assert_eq!(c.idle_resync_inflight_op_id, 0);

    let joined = near_sdk::test_utils::get_logs().join("\n");
    assert!(joined.contains("\"event\":\"idle_resync_completed\""));
}

#[test]
fn idle_resync_callback_balance_read_failure_stops_and_clears_inflight() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    c.idle_resync_last_ns = 0;
    c.idle_resync_cooldown_ns = 120_000_000_000;
    c.idle_resync_inflight_op_id = 0;

    let caller = mk(1);

    let op_id = 42u64;
    c.idle_resync_inflight_op_id = op_id;
    c.op_state = OpState::Allocating(AllocatingState {
        op_id,
        index: 0,
        remaining: 0,
        plan: vec![],
    });

    c.idle_balance = 100;
    c.fee_anchor.total_assets = 1_000.into();
    c.fee_anchor.timestamp_ns = 777.into();

    let finished_at_ns = 1_234u64;
    set_ctx_with_gas(&vault_id, &caller, Some(finished_at_ns), None, None);

    let report = c.resync_idle_balance_01_settle(
        Err(near_sdk::PromiseError::Failed),
        op_id,
        caller.clone(),
        U128(100),
        0,
    );

    assert_eq!(report.outcome, IdleResyncOutcome::BalanceReadFailed);
    assert_eq!(report.before_idle, U128(100));
    assert_eq!(report.actual_idle, U128(100));
    assert_eq!(report.after_idle, U128(100));
    assert_eq!(report.increased_by, U128(0));
    assert_eq!(report.decreased_by, U128(0));
    assert_eq!(report.fee_anchor_bump, U128(0));
    assert_eq!(report.resynced_at_ns.0, finished_at_ns);

    assert_eq!(c.idle_balance, 100);
    assert_eq!(c.fee_anchor.total_assets.0, 1_000);
    assert_eq!(c.fee_anchor.timestamp_ns.0, 777);
    assert!(matches!(c.op_state, OpState::Idle));
    assert_eq!(c.idle_resync_inflight_op_id, 0);

    let joined = near_sdk::test_utils::get_logs().join("\n");
    assert!(
        joined.contains("\"event\":\"idle_resync_stopped\"")
            && joined.contains("balance read failed")
    );
}

#[test]
fn idle_resync_callback_mismatched_op_id_is_ignored_without_mutating_state() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    c.idle_resync_last_ns = 0;
    c.idle_resync_cooldown_ns = 120_000_000_000;
    c.idle_resync_inflight_op_id = 0;

    let caller = mk(1);

    c.idle_resync_inflight_op_id = 999;
    c.op_state = OpState::Allocating(AllocatingState {
        op_id: 999,
        index: 0,
        remaining: 0,
        plan: vec![],
    });

    c.idle_balance = 100;
    c.fee_anchor.total_assets = 1_000.into();
    c.fee_anchor.timestamp_ns = 777.into();

    let finished_at_ns = 1_234u64;
    set_ctx_with_gas(&vault_id, &caller, Some(finished_at_ns), None, None);

    let report = c.resync_idle_balance_01_settle(Ok(U128(150)), 888, caller.clone(), U128(100), 0);

    assert_eq!(report.outcome, IdleResyncOutcome::Ignored);
    assert_eq!(c.idle_balance, 100);
    assert_eq!(c.fee_anchor.total_assets.0, 1_000);
    assert_eq!(c.fee_anchor.timestamp_ns.0, 777);
    assert_eq!(c.idle_resync_inflight_op_id, 999);
    assert!(matches!(c.op_state, OpState::Allocating(_)));

    let joined = near_sdk::test_utils::get_logs().join("\n");
    assert!(joined.contains("\"event\":\"idle_resync_callback_ignored\""));
}

#[test]
fn idle_resync_callback_unexpected_state_clears_inflight_and_returns_idle() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    c.idle_resync_last_ns = 0;
    c.idle_resync_cooldown_ns = 120_000_000_000;
    c.idle_resync_inflight_op_id = 0;

    let caller = mk(1);

    let op_id = 42u64;
    c.idle_resync_inflight_op_id = op_id;
    c.op_state = OpState::Idle;

    c.idle_balance = 100;
    c.fee_anchor.total_assets = 1_000.into();
    c.fee_anchor.timestamp_ns = 777.into();

    let finished_at_ns = 1_234u64;
    set_ctx_with_gas(&vault_id, &caller, Some(finished_at_ns), None, None);

    let report =
        c.resync_idle_balance_01_settle(Ok(U128(150)), op_id, caller.clone(), U128(100), 0);

    assert_eq!(report.outcome, IdleResyncOutcome::UnexpectedState);
    assert_eq!(c.idle_balance, 100);
    assert_eq!(c.fee_anchor.total_assets.0, 1_000);
    assert_eq!(c.fee_anchor.timestamp_ns.0, 777);
    assert!(matches!(c.op_state, OpState::Idle));
    assert_eq!(c.idle_resync_inflight_op_id, 0);

    let joined = near_sdk::test_utils::get_logs().join("\n");
    assert!(
        joined.contains("\"event\":\"idle_resync_stopped\"")
            && joined.contains("IdleResyncUnexpectedState")
    );
}

#[test]
fn idle_resync_cooldown_throttles_and_allows_after_cooldown() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    c.idle_resync_last_ns = 0;
    c.idle_resync_cooldown_ns = 120_000_000_000;
    c.idle_resync_inflight_op_id = 0;

    let caller = mk(1);

    let mut logs = String::new();

    let cooldown = c.idle_resync_cooldown_ns;
    let t0 = cooldown.saturating_add(1);

    set_ctx_with_gas(
        &vault_id,
        &caller,
        Some(t0),
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );

    let op_id = c.next_op_id;
    let _ = c.resync_idle_balance();
    logs.push_str(&near_sdk::test_utils::get_logs().join("\n"));

    set_ctx_with_gas(&vault_id, &caller, Some(t0.saturating_add(5)), None, None);
    let _ = c.resync_idle_balance_01_settle(Ok(U128(0)), op_id, caller.clone(), U128(0), t0);
    logs.push_str(&near_sdk::test_utils::get_logs().join("\n"));

    set_ctx_with_gas(
        &vault_id,
        &caller,
        Some(t0.saturating_add(1)),
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );
    let throttled =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.resync_idle_balance()));
    let throttled_msg = throttled
        .err()
        .map(panic_payload_to_string)
        .unwrap_or_default();
    assert!(throttled_msg.contains("Idle resync throttled"));

    set_ctx_with_gas(
        &vault_id,
        &caller,
        Some(t0.saturating_add(cooldown)),
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );

    let op_id2 = c.next_op_id;
    let _ = c.resync_idle_balance();
    logs.push_str(&near_sdk::test_utils::get_logs().join("\n"));

    set_ctx_with_gas(
        &vault_id,
        &caller,
        Some(t0.saturating_add(cooldown).saturating_add(5)),
        None,
        None,
    );
    let _ = c.resync_idle_balance_01_settle(
        Ok(U128(0)),
        op_id2,
        caller.clone(),
        U128(0),
        t0.saturating_add(cooldown),
    );
    logs.push_str(&near_sdk::test_utils::get_logs().join("\n"));

    assert!(logs.contains("\"event\":\"idle_resync_started\""));
    assert!(logs.contains("\"event\":\"idle_resync_completed\""));
}

#[test]
#[should_panic(expected = "Cannot deposit during idle resync")]
fn execute_supply_is_blocked_during_idle_resync() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    c.idle_resync_last_ns = 0;
    c.idle_resync_cooldown_ns = 120_000_000_000;
    c.idle_resync_inflight_op_id = 0;

    c.idle_resync_inflight_op_id = 1;

    let sender = mk(1);
    let asset_id = c.underlying_asset.contract_id().into();
    let _ = c.execute_supply(sender, asset_id, 1);
}

#[test]
#[should_panic(expected = "Cannot withdraw/redeem during idle resync")]
fn withdraw_is_blocked_during_idle_resync() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    c.idle_resync_last_ns = 0;
    c.idle_resync_cooldown_ns = 120_000_000_000;
    c.idle_resync_inflight_op_id = 0;

    c.idle_resync_inflight_op_id = 1;

    let caller = mk(1);
    set_ctx_with_gas(
        &vault_id,
        &caller,
        None,
        None,
        Some(near_sdk::Gas::from_tgas(300)),
    );
    let _ = c.withdraw(U128(1), mk(2));
}

#[test]
#[should_panic(expected = "Cannot withdraw/redeem during idle resync")]
fn redeem_is_blocked_during_idle_resync() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    c.idle_resync_last_ns = 0;
    c.idle_resync_cooldown_ns = 120_000_000_000;
    c.idle_resync_inflight_op_id = 0;

    c.idle_resync_inflight_op_id = 1;

    let caller = mk(1);
    set_ctx_with_gas(
        &vault_id,
        &caller,
        None,
        Some(crate::storage_management::yocto_for_ft_account()),
        None,
    );
    let _ = c.redeem(U128(1), mk(2));
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
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let m = mk(1);
    let cfg = MarketConfiguration {
        cap: U128(cap),
        enabled: cap > 0,
        removable_at: TimestampNs::ZERO,
        cap_group_id: None,
    };
    let market_id = c.insert_market_for_tests(m.clone(), cfg, cur);
    c.supply_queue.push(market_id);
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
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let mut c = new_test_contract(&vault_id);

    let _ = c.insert_market_for_tests(mk(7003), MarketConfiguration::default(), principal);
    c.idle_balance = idle;

    assert_eq!(c.get_total_assets().0, idle.saturating_add(principal));
}

#[rstest]
fn set_fees_accrues_before_switching_recipient(owner_env: OwnerEnv) {
    let OwnerEnv { mut contract, .. } = owner_env;

    // Seed supply so fee shares can mint
    contract
        .deposit_unchecked(&mk(1), 1_000)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));
    // Simulate profit: last=1000, current=1500
    contract.idle_balance = 1_500;
    contract.fee_anchor.total_assets = 1_000.into();
    contract.fee_anchor.timestamp_ns = env::block_timestamp().into();
    contract.fees.performance.fee = Wad::one() / 10;

    let cur = contract.get_total_assets().0;
    let ts_before = contract.total_supply();
    let expect = compute_fee_shares(
        cur.into(),
        1_000.into(),
        contract.fees.performance.fee,
        ts_before.into(),
    );

    let old_recipient = contract.fees.performance.recipient.clone();
    let old_balance = contract.balance_of(&old_recipient);

    // Switch fee recipient; should accrue to old recipient first.
    let new_recipient = mk(3);
    contract.set_fees(build_fees(
        contract.fees.performance.fee,
        contract.fees.management.fee,
        new_recipient.clone(),
        new_recipient.clone(),
    ));

    assert_eq!(
        contract.governance_timelocks.pending_len(),
        1,
        "recipient change should be timelocked"
    );
    assert_eq!(
        contract.fees.performance.recipient.clone(),
        old_recipient,
        "recipient should not change until accept"
    );

    contract.accept_fees();

    assert_eq!(
        contract.balance_of(&old_recipient),
        old_balance + expect.as_u128_trunc(),
        "fees must accrue to the old recipient before switching"
    );
    assert_eq!(
        contract.total_supply(),
        ts_before + expect.as_u128_trunc(),
        "total supply must increase by minted fee shares"
    );
    assert_eq!(
        contract.fees.performance.recipient.clone(),
        new_recipient,
        "recipient should be updated"
    );
    assert_eq!(
        contract.fee_anchor.total_assets.0, cur,
        "fee anchor must update to current after accrual"
    );
}

#[rstest]
fn set_fees_accrues_before_switching_recipient_variant(owner_env: OwnerEnv) {
    let OwnerEnv { mut contract, .. } = owner_env;

    // Seed supply so fee shares can mint
    contract
        .deposit_unchecked(&mk(2), 2_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));

    // Simulate profit: last=2000, current=2400
    contract.idle_balance = 2_400;
    contract.fee_anchor.total_assets = 2_000.into();
    contract.fee_anchor.timestamp_ns = env::block_timestamp().into();
    contract.fees.performance.fee = Wad::one() / 20;

    let cur = contract.get_total_assets().0;
    let ts_before = contract.total_supply();
    let expect = compute_fee_shares(
        cur.into(),
        2_000.into(),
        contract.fees.performance.fee,
        ts_before.into(),
    );

    let old_recipient = contract.fees.performance.recipient.clone();
    let old_balance = contract.balance_of(&old_recipient);

    // Switch fee recipient; should accrue to old recipient first.
    let new_recipient = mk(3);
    contract.set_fees(build_fees(
        contract.fees.performance.fee,
        contract.fees.management.fee,
        new_recipient.clone(),
        new_recipient.clone(),
    ));

    assert_eq!(
        contract.governance_timelocks.pending_len(),
        1,
        "recipient change should be timelocked"
    );
    assert_eq!(
        contract.fees.performance.recipient.clone(),
        old_recipient,
        "recipient should not change until accept"
    );

    contract.accept_fees();

    assert_eq!(
        contract.balance_of(&old_recipient),
        old_balance + expect.as_u128_trunc(),
        "fees must accrue to the old recipient before switching"
    );
    assert_eq!(
        contract.total_supply(),
        ts_before + expect.as_u128_trunc(),
        "total supply must increase by minted fee shares"
    );
    assert_eq!(
        contract.fees.performance.recipient, new_recipient,
        "recipient should be updated"
    );

    assert_eq!(
        contract.fee_anchor.total_assets.0, cur,
        "fee anchor must update to current after accrual"
    );
}

#[rstest]
fn set_fees_increase_is_timelocked(owner_env: OwnerEnv) {
    let OwnerEnv { mut contract, .. } = owner_env;

    contract.fees.performance.fee = Wad::one() / 100;
    let before = contract.fees.performance.fee;

    let increased = Wad::one() / 10;

    contract.set_fees(build_fees(
        increased,
        contract.fees.management.fee,
        contract.fees.performance.recipient.clone(),
        contract.fees.management.recipient.clone(),
    ));

    assert_eq!(
        contract.governance_timelocks.pending_len(),
        1,
        "fee increase should be timelocked"
    );
    assert_eq!(
        contract.fees.performance.fee, before,
        "fee should not change until accept"
    );

    contract.accept_fees();

    assert_eq!(contract.governance_timelocks.pending_len(), 0);
    assert_eq!(contract.fees.performance.fee, increased);
}

#[rstest]
fn set_fees_accrues_with_old_rate_then_updates_performance(owner_env: OwnerEnv) {
    let OwnerEnv { mut contract, .. } = owner_env;

    // Seed supply so fee shares can mint
    contract
        .deposit_unchecked(&mk(1), 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));

    // Simulate profit: last=1000, current=1500
    contract.idle_balance = 1_500;
    contract.fee_anchor.total_assets = 1_000.into();
    contract.fee_anchor.timestamp_ns = env::block_timestamp().into();

    // Old rate = 10%, new rate = 1%
    contract.fees.performance.fee = Wad::one() / 10;
    let cur = contract.get_total_assets().0;
    let ts_before = contract.total_supply();
    let expect_old = compute_fee_shares(
        cur.into(),
        1_000.into(),
        contract.fees.performance.fee,
        ts_before.into(),
    );

    let recipient = contract.fees.performance.recipient.clone();
    let bal_before = contract.balance_of(&recipient);

    contract.set_fees(build_fees(
        Wad::one() / 100,
        contract.fees.management.fee,
        contract.fees.performance.recipient.clone(),
        contract.fees.management.recipient.clone(),
    ));

    assert_eq!(
        contract.balance_of(&recipient),
        bal_before + expect_old.as_u128_trunc(),
        "accrual must use the old fee rate before updating",
    );

    assert_eq!(
        contract.fees.performance.fee,
        Wad::one() / 100,
        "performance fee must be updated to the new rate"
    );

    assert_eq!(
        contract.total_supply(),
        ts_before + expect_old.as_u128_trunc(),
        "total supply must reflect fee shares minted at old rate"
    );
    assert_eq!(
        contract.fees.performance.fee,
        Wad::one() / 100,
        "performance fee must be updated to the new rate"
    );
    assert_eq!(
        contract.fee_anchor.total_assets.0, cur,
        "fee anchor must update to current after accrual"
    );
}

#[rstest]
fn set_fees_accrues_with_old_rate_then_updates_performance_variant(owner_env: OwnerEnv) {
    let OwnerEnv { mut contract, .. } = owner_env;

    // Seed supply so fee shares can mint
    contract
        .deposit_unchecked(&mk(2), 2_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));

    // Simulate profit: last=2000, current=2400
    contract.idle_balance = 2_400;
    contract.fee_anchor.total_assets = 2_000.into();
    contract.fee_anchor.timestamp_ns = env::block_timestamp().into();

    // Old rate = 5%, new rate = 0.5%
    contract.fees.performance.fee = Wad::one() / 20;
    let cur = contract.get_total_assets().0;
    let ts_before = contract.total_supply();
    let expect_old = compute_fee_shares(
        cur.into(),
        2_000.into(),
        contract.fees.performance.fee,
        ts_before.into(),
    );

    let recipient = contract.fees.performance.recipient.clone();
    let bal_before = contract.balance_of(&recipient);

    contract.set_fees(build_fees(
        Wad::one() / 200,
        contract.fees.management.fee,
        contract.fees.performance.recipient.clone(),
        contract.fees.management.recipient.clone(),
    ));

    assert_eq!(
        contract.balance_of(&recipient),
        bal_before + expect_old.as_u128_trunc(),
        "accrual must use the old fee rate before updating",
    );

    assert_eq!(
        contract.fees.performance.fee,
        Wad::one() / 200,
        "performance fee must be updated to the new rate"
    );

    assert_eq!(
        contract.total_supply(),
        ts_before + expect_old.as_u128_trunc(),
        "total supply must reflect fee shares minted at old rate"
    );
    assert_eq!(
        contract.fees.performance.fee,
        Wad::one() / 200,
        "performance fee must be updated to the new rate"
    );
    assert_eq!(
        contract.fee_anchor.total_assets.0, cur,
        "fee anchor must update to current after accrual"
    );
}

#[rstest]
#[should_panic(expected = "management fee too high")]
fn set_fees_rejects_management_fee_above_cap(owner_env: OwnerEnv) {
    let OwnerEnv { mut contract, .. } = owner_env;

    contract.set_fees(build_fees(
        contract.fees.performance.fee,
        Wad::from(MAX_MANAGEMENT_FEE_WAD + 1),
        contract.fees.performance.recipient.clone(),
        contract.fees.management.recipient.clone(),
    ));
}

#[rstest]
#[should_panic(expected = "performance fee too high")]
fn set_fees_rejects_performance_fee_above_cap(owner_env: OwnerEnv) {
    let OwnerEnv { mut contract, .. } = owner_env;

    contract.set_fees(build_fees(
        Wad::from(MAX_PERFORMANCE_FEE_WAD + 1),
        contract.fees.management.fee,
        contract.fees.performance.recipient.clone(),
        contract.fees.management.recipient.clone(),
    ));
}

#[rstest]
fn restrictions_pause_is_immediate_for_sentinel(owner_env: OwnerEnv) {
    let OwnerEnv {
        vault_id,
        mut contract,
        ..
    } = owner_env;

    let sentinel = contract.get_configuration().sentinel;

    set_ctx(&vault_id, &sentinel, None, None);
    contract.set_restrictions(Some(Restrictions::Paused));

    assert!(matches!(
        contract.get_restrictions(),
        Some(Restrictions::Paused)
    ));
    assert_eq!(contract.governance_timelocks.pending_len(), 0);
}

#[rstest]
fn restrictions_unpause_is_timelocked(owner_env: OwnerEnv) {
    let OwnerEnv { mut contract, .. } = owner_env;

    // Emergency pause applies immediately.
    contract.set_restrictions(Some(Restrictions::Paused));
    assert!(matches!(
        contract.get_restrictions(),
        Some(Restrictions::Paused)
    ));

    // Unpause is a relax, so it must be timelocked.
    contract.set_restrictions(None);
    assert!(matches!(
        contract.get_restrictions(),
        Some(Restrictions::Paused)
    ));
    assert_eq!(contract.governance_timelocks.pending_len(), 1);

    contract.accept_restrictions();

    assert!(contract.get_restrictions().is_none());
    assert_eq!(contract.governance_timelocks.pending_len(), 0);
}

#[rstest]
fn restrictions_unpause_by_sentinel_is_timelocked(owner_env: OwnerEnv) {
    let OwnerEnv {
        vault_id,
        mut contract,
        ..
    } = owner_env;

    contract.set_restrictions(Some(Restrictions::Paused));

    let sentinel = contract.get_configuration().sentinel;
    set_ctx(&vault_id, &sentinel, None, None);

    contract.set_restrictions(None);

    assert!(matches!(
        contract.get_restrictions(),
        Some(Restrictions::Paused)
    ));
    assert_eq!(contract.governance_timelocks.pending_len(), 1);
}

#[rstest]
#[should_panic(expected = "management fee too high")]
fn init_rejects_management_fee_above_cap(vault_id: AccountId) {
    setup_env(&vault_id, &vault_id, vec![]);

    // Basic accounts
    let owner = mk(1);
    let curator = mk(2);
    let guardian = mk(3);
    let sentinel = mk(7);
    let fee_recipient = mk(4);
    let skim_recipient = mk(5);
    let underlying_token_id = mk(6);

    let mut cfg = ::test_utils::vault_configuration(
        owner,
        curator,
        guardian,
        sentinel,
        underlying_token_id,
        skim_recipient,
        fee_recipient,
    );

    cfg.fees.management.fee = Wad::from(MAX_MANAGEMENT_FEE_WAD + 1);

    let _ = Contract::new(cfg);
}

#[rstest]
#[should_panic(expected = "performance fee too high")]
fn init_rejects_performance_fee_above_cap(vault_id: AccountId) {
    setup_env(&vault_id, &vault_id, vec![]);

    // Basic accounts
    let owner = mk(1);
    let curator = mk(2);
    let guardian = mk(3);
    let sentinel = mk(7);
    let fee_recipient = mk(4);
    let skim_recipient = mk(5);
    let underlying_token_id = mk(6);

    let mut cfg = ::test_utils::vault_configuration(
        owner,
        curator,
        guardian,
        sentinel,
        underlying_token_id,
        skim_recipient,
        fee_recipient,
    );

    cfg.fees.performance.fee = Wad::from(MAX_PERFORMANCE_FEE_WAD + 1);

    let _ = Contract::new(cfg);
}

#[rstest]
fn management_fee_accrues_proportionally(owner_env: OwnerEnv) {
    let OwnerEnv {
        vault_id,
        mut contract,
        ..
    } = owner_env;

    contract.fees.management.fee = Wad::one() / 20;
    contract.fees.performance.fee = Wad::zero();

    contract
        .deposit_unchecked(&accounts(1), 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    contract.idle_balance = 1_000;

    let initial_supply = contract.total_supply();
    let cur_assets = contract.get_total_assets().0;
    let recipient = contract.fees.management.recipient.clone();
    let elapsed = YEAR_NS / 2;

    set_block_ts(&vault_id, &vault_id, elapsed);
    contract.internal_accrue_fee();

    let expected_fee_assets = mul_div_floor(
        contract
            .fees
            .management
            .fee
            .apply_floored(Number::from(cur_assets)),
        Number::from(u128::from(elapsed)),
        Number::from(u128::from(YEAR_NS)),
    );
    let expected_shares = compute_fee_shares_from_assets(
        expected_fee_assets,
        cur_assets.into(),
        initial_supply.into(),
    );

    assert_eq!(
        contract.total_supply(),
        initial_supply + expected_shares.as_u128_trunc(),
        "management fee shares must mint proportionally to elapsed time",
    );
    assert_eq!(
        contract.balance_of(&recipient),
        expected_shares.as_u128_trunc(),
        "management fee recipient must receive minted shares",
    );
    assert_eq!(
        contract.fee_anchor.total_assets.0, cur_assets,
        "fee anchor assets must remain unchanged after management accrual",
    );
    assert_eq!(
        contract.fee_anchor.timestamp_ns.0, elapsed,
        "fee anchor timestamp must update to now after accrual",
    );
}

#[rstest]
fn management_fee_zero_elapsed_is_noop(owner_env: OwnerEnv) {
    let OwnerEnv {
        vault_id,
        mut contract,
        ..
    } = owner_env;

    contract.fees.management.fee = Wad::one() / 20;
    contract.fees.performance.fee = Wad::zero();

    contract
        .deposit_unchecked(&accounts(1), 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    contract.idle_balance = 1_000;

    let initial_supply = contract.total_supply();
    let recipient = contract.fees.management.recipient.clone();
    let bal_before = contract.balance_of(&recipient);

    set_block_ts(&vault_id, &vault_id, env::block_timestamp());
    contract.internal_accrue_fee();

    assert_eq!(
        contract.total_supply(),
        initial_supply,
        "no mint when elapsed is zero"
    );
    assert_eq!(
        contract.balance_of(&recipient),
        bal_before,
        "recipient balance unchanged when elapsed is zero"
    );
}

#[rstest]
fn management_fee_is_rate_limited_by_max_total_assets_growth_rate(owner_env: OwnerEnv) {
    let OwnerEnv {
        vault_id,
        mut contract,
        ..
    } = owner_env;

    // Seed supply so total_supply > 0
    contract
        .deposit_unchecked(&mk(1), 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));

    contract.fees.performance.fee = Wad::zero();
    contract.fees.management.fee = Wad::one() / 20; // 5% annual
    contract.fees.max_total_assets_growth_rate = Some(Wad::one() / 5); // 20% annual

    // last=1_000, cur=2_000
    contract.idle_balance = 2_000;
    contract.fee_anchor.total_assets = 1_000.into();
    contract.fee_anchor.timestamp_ns = 0.into();

    let ts_before = contract.total_supply();
    let recipient = contract.fees.management.recipient.clone();
    let bal_before = contract.balance_of(&recipient);

    // Half a year elapsed: max allowed growth = 1_000 * 20% * 0.5 = 100
    let elapsed = YEAR_NS / 2;
    set_block_ts(&vault_id, &vault_id, elapsed);

    contract.internal_accrue_fee();

    let annual_max_increase = contract
        .fees
        .max_total_assets_growth_rate
        .expect("max rate")
        .apply_floored(Number::from(1_000u128));
    let max_increase = mul_div_floor(
        annual_max_increase,
        Number::from(u128::from(elapsed)),
        Number::from(u128::from(YEAR_NS)),
    );
    let fee_total_assets = 1_000u128.saturating_add(max_increase.as_u128_trunc());

    let annual_fee_assets = contract
        .fees
        .management
        .fee
        .apply_floored(Number::from(fee_total_assets));
    let fee_assets = mul_div_floor(
        annual_fee_assets,
        Number::from(u128::from(elapsed)),
        Number::from(u128::from(YEAR_NS)),
    );
    let expected_shares = compute_fee_shares_from_assets(
        fee_assets,
        Number::from(2_000u128),
        Number::from(ts_before),
    );

    let minted = expected_shares.as_u128_trunc();

    assert_eq!(
        contract.total_supply(),
        ts_before + minted,
        "total supply must reflect minted management fee shares",
    );
    assert_eq!(
        contract.balance_of(&recipient),
        bal_before + minted,
        "management fee shares should be rate-limited",
    );
    assert_eq!(
        contract.fee_anchor.total_assets,
        2_000.into(),
        "fee anchor must sync to current after accrual",
    );
}

#[rstest]
fn performance_fee_is_rate_limited_by_max_total_assets_growth_rate(owner_env: OwnerEnv) {
    let OwnerEnv {
        vault_id,
        mut contract,
        ..
    } = owner_env;

    // Seed supply so total_supply > 0
    contract
        .deposit_unchecked(&mk(1), 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));

    contract.fees.management.fee = Wad::zero();
    contract.fees.performance.fee = Wad::one() / 2; // 50% performance fee
    contract.fees.max_total_assets_growth_rate = Some(Wad::one() / 5); // 20% annual

    // last=1_000, cur=2_000
    contract.idle_balance = 2_000;
    contract.fee_anchor.total_assets = 1_000.into();
    contract.fee_anchor.timestamp_ns = 0.into();

    let ts_before = contract.total_supply();
    let recipient = contract.fees.performance.recipient.clone();
    let bal_before = contract.balance_of(&recipient);

    // 0.1 year elapsed: max allowed growth = 1_000 * 20% * 0.1 = 20
    let elapsed = YEAR_NS / 10;
    set_block_ts(&vault_id, &vault_id, elapsed);

    contract.internal_accrue_fee();

    let annual_max_increase = contract
        .fees
        .max_total_assets_growth_rate
        .expect("max rate")
        .apply_floored(Number::from(1_000u128));
    let max_increase = mul_div_floor(
        annual_max_increase,
        Number::from(u128::from(elapsed)),
        Number::from(u128::from(YEAR_NS)),
    );
    let fee_total_assets = 1_000u128.saturating_add(max_increase.as_u128_trunc());

    let profit = fee_total_assets.saturating_sub(1_000);
    let fee_assets = contract
        .fees
        .performance
        .fee
        .apply_floored(Number::from(profit));
    let expected_shares = compute_fee_shares_from_assets(
        fee_assets,
        Number::from(2_000u128),
        Number::from(ts_before),
    );

    let minted = expected_shares.as_u128_trunc();

    assert_eq!(
        contract.total_supply(),
        ts_before + minted,
        "total supply must reflect minted performance fee shares",
    );
    assert_eq!(
        contract.balance_of(&recipient),
        bal_before + minted,
        "performance fee shares should be rate-limited",
    );
    assert_eq!(
        contract.fee_anchor.total_assets,
        2_000.into(),
        "fee anchor must sync to current after accrual",
    );
}

#[rstest]
fn internal_accrue_fee_mints_zero_on_loss_and_updates_last(owner_env: OwnerEnv) {
    let OwnerEnv { mut contract, .. } = owner_env;

    // Seed supply so total_supply > 0
    contract
        .deposit_unchecked(&mk(1), 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));

    // Loss scenario: last=1000, current=800
    contract.idle_balance = 800;
    contract.fee_anchor.total_assets = 1_000.into();
    contract.fee_anchor.timestamp_ns = env::block_timestamp().into();
    contract.fees.performance.fee = Wad::one() / 10;

    let ts_before = contract.total_supply();
    let fr = contract.fees.performance.recipient.clone();
    let bal_before = contract.balance_of(&fr);
    let cur = contract.get_total_assets().0;

    contract.internal_accrue_fee();

    assert_eq!(
        contract.total_supply(),
        ts_before,
        "no shares should be minted when cur < last_total_assets"
    );
    assert_eq!(
        contract.balance_of(&fr),
        bal_before,
        "fee recipient balance must remain unchanged on loss"
    );
    assert_eq!(
        contract.fee_anchor.total_assets,
        cur.into(),
        "fee anchor must update to current after accrual"
    );
}

#[rstest]
fn ft_on_transfer_supply_accepts_full_and_mints_shares(
    c_asset_env: Contract,
    enabled_market_100: (AccountId, MarketConfiguration),
) {
    let mut c = c_asset_env;

    let (m, cfg) = enabled_market_100;
    let market_id = c.insert_market_for_tests(m, cfg, 0);
    c.supply_queue.push(market_id);

    let sender = mk(1);
    let deposit = 50u128;
    let expect_shares = c.preview_deposit(U128(deposit)).0;

    let res = c.ft_on_transfer(sender.clone(), U128(deposit), supply_msg());
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
        c.fee_anchor.total_assets,
        deposit.into(),
        "fee anchor must increase by accepted deposit"
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

    let (m, mut cfg) = enabled_market_100;
    cfg.cap = U128(50); // override cap for this case
    let market_id = c.insert_market_for_tests(m, cfg, 0);
    c.supply_queue.push(market_id);

    let sender = mk(2);
    let deposit = 80u128;
    let accept = 50u128;
    let expect_shares = c.preview_deposit(U128(accept)).0;

    let res = c.ft_on_transfer(sender.clone(), U128(deposit), supply_msg());
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
        c.fee_anchor.total_assets,
        accept.into(),
        "fee anchor increases by accepted amount only"
    );
}

#[test]
fn cap_group_limits_total_room() {
    let vault_id = accounts(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let group = CapGroupId::try_from("group-a".to_string()).unwrap();
    c.cap_groups
        .insert(group.clone(), cap_group_record(150, Wad::one(), 0));

    for offset in 0..2 {
        let cfg = MarketConfiguration {
            cap: U128(100),
            enabled: true,
            removable_at: TimestampNs::ZERO,
            cap_group_id: Some(group.clone()),
        };
        let market_id = c.insert_market_for_tests(mk(9200 + offset), cfg, 0);
        c.supply_queue.push(market_id);
    }

    assert_eq!(c.get_max_single_market_deposit().0, 100);
    assert_eq!(c.get_max_deposit().0, 150);
}

#[test]
fn cap_group_relative_caps_scale_with_aum() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let asset: AccountId = c.underlying_asset.contract_id().into();
    setup_env(&vault_id, &asset, vec![]);

    let half = Wad::one() / 2;

    let group_a = CapGroupId::try_from("group-ra".to_string()).unwrap();
    let group_b = CapGroupId::try_from("group-rb".to_string()).unwrap();

    c.cap_groups
        .insert(group_a.clone(), cap_group_record(10_000, half, 0));
    c.cap_groups
        .insert(group_b.clone(), cap_group_record(10_000, half, 0));

    let m1_id = c.insert_market_for_tests(
        mk(9310),
        MarketConfiguration {
            cap: U128(1_000),
            enabled: true,
            removable_at: TimestampNs::ZERO,
            cap_group_id: Some(group_a.clone()),
        },
        0,
    );
    c.supply_queue.push(m1_id);

    let m2_id = c.insert_market_for_tests(
        mk(9311),
        MarketConfiguration {
            cap: U128(1_000),
            enabled: true,
            removable_at: TimestampNs::ZERO,
            cap_group_id: Some(group_b.clone()),
        },
        0,
    );
    c.supply_queue.push(m2_id);

    assert_eq!(c.get_max_deposit().0, 2_000);

    let sender = mk(1);
    let res = c.ft_on_transfer(sender, U128(3_000), supply_msg());
    match res {
        PromiseOrValue::Value(U128(refund)) => assert_eq!(refund, 1_000),
        _ => panic!("expected refund"),
    }

    assert_eq!(c.idle_balance, 2_000);
}

#[test]
fn cap_group_refunds_when_saturated() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let asset: AccountId = c.underlying_asset.contract_id().into();
    setup_env(&vault_id, &asset, vec![]);

    let group = CapGroupId::try_from("group-b".to_string()).unwrap();
    c.cap_groups
        .insert(group.clone(), cap_group_record(50, Wad::one(), 0));

    let market_account = mk(9300);
    let cfg = MarketConfiguration {
        cap: U128(100),
        enabled: true,
        removable_at: TimestampNs::ZERO,
        cap_group_id: Some(group.clone()),
    };
    let market_id = c.insert_market_for_tests(market_account, cfg, 0);
    c.supply_queue.push(market_id);
    c.set_market_principal(market_id, 50);

    assert_eq!(c.get_max_deposit().0, 0);

    let sender = accounts(5);
    let res = c.ft_on_transfer(sender, U128(25), supply_msg());
    match res {
        PromiseOrValue::Value(U128(refund)) => assert_eq!(refund, 25),
        _ => panic!("expected refund"),
    }

    assert_eq!(c.idle_balance, 0);
    assert_eq!(c.get_max_deposit().0, 0);
}

#[test]
#[should_panic = "Invalid token ID"]
fn ft_on_transfer_wrong_token_full_refund_via_receiver() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&mk(42));
    setup_env(&vault_id, &vault_id, vec![]);

    let market_account = mk(9003);
    let cfg = MarketConfiguration {
        cap: U128(100),
        enabled: true,
        removable_at: TimestampNs::ZERO,
        cap_group_id: None,
    };
    let market_id = c.insert_market_for_tests(market_account, cfg, 0);
    c.supply_queue.push(market_id);

    let sender = mk(3);
    let deposit = 70u128;

    let res = c.ft_on_transfer(sender.clone(), U128(deposit), supply_msg());
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
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &vault_id, vec![]);

    let _ = c.ft_on_transfer(mk(4), U128(10), "not-json".into());
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

    let (market_account, cfg) = enabled_market_100;
    let market_id = c.insert_market_for_tests(market_account, cfg, 0);
    c.supply_queue.push(market_id);

    let sender: AccountId = c.underlying_asset.contract_id().into();

    c.ft_on_transfer(sender.clone(), U128(0), supply_msg())
        .detach();
}

#[test]
#[should_panic = "Invalid deposit msg"]
fn mt_on_transfer_invalid_msg_panics() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &vault_id, vec![]);

    let _ = c.mt_on_transfer(
        mk(1),
        vec![mk(1)],
        vec!["t".to_string()],
        vec![U128(1)],
        "bad".into(),
    );
}

#[test]
#[should_panic = "This contract only accepts one token at a time."]
fn mt_on_transfer_rejects_multiple_tokens() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &vault_id, vec![]);

    let _ = c.mt_on_transfer(
        mk(2),
        vec![mk(2)],
        vec!["a".to_string(), "b".to_string()],
        vec![U128(1)],
        supply_msg(),
    );
}

#[test]
#[should_panic = "Invalid input length"]
fn mt_on_transfer_rejects_invalid_input_lengths() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &vault_id, vec![]);

    let _ = c.mt_on_transfer(
        mk(3),
        vec![mk(3), mk(4)],
        vec!["t".to_string()],
        vec![U128(1)],
        supply_msg(),
    );
}

#[test]
fn mt_on_transfer_wrong_asset_refunds_full() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let old_ft_id = c.underlying_asset.contract_id().into();
    setup_env(&vault_id, &old_ft_id, vec![]);

    let token_id = "token-1".to_string();

    c.underlying_asset = FungibleAsset::nep245(old_ft_id.clone(), token_id.clone());

    let sender = mk(5);
    let amount = 25u128;

    let res = c.mt_on_transfer(
        mk(3),
        vec![sender.clone()],
        vec![token_id],
        vec![U128(amount)],
        supply_msg(),
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
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    setup_env(&vault_id, &vault_id, vec![]);

    let asset_id = c.underlying_asset.contract_id().into();
    let sender_id = mk(4);
    c.execute_supply(sender_id.clone(), asset_id, 0);
}

#[test]
fn governance_set_curator_grants_allocator() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let market_account = mk(9101);
    let cfg = MarketConfiguration {
        cap: U128(1),
        enabled: true,
        removable_at: TimestampNs::ZERO,
        cap_group_id: None,
    };
    let market_id = c.insert_market_for_tests(market_account, cfg, 0);

    let new_cur = mk(3);
    c.set_curator(new_cur.clone());

    set_ctx(
        &vault_id,
        &new_cur,
        None,
        Some(yocto_for_bytes(storage_bytes_for_queue_account_id())),
    );
    c.set_supply_queue(vec![market_id]);
    assert_eq!(c.supply_queue.len(), 1);
    assert_eq!(c.supply_queue.iter().next(), Some(&market_id));
}

#[test]
fn governance_set_is_allocator_grant_allows_queue_ops() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let grantee = mk(4);

    let market_account = mk(9102);
    let cfg = MarketConfiguration {
        cap: U128(1),
        enabled: true,
        removable_at: TimestampNs::ZERO,
        cap_group_id: None,
    };
    let market_id = c.insert_market_for_tests(market_account, cfg, 0);

    c.set_is_allocator(grantee.clone(), true);

    set_ctx(
        &vault_id,
        &grantee,
        None,
        Some(yocto_for_bytes(storage_bytes_for_queue_account_id())),
    );
    c.set_supply_queue(vec![market_id]);
    assert_eq!(c.supply_queue.len(), 1);
    assert_eq!(c.supply_queue.iter().next(), Some(&market_id));
}

#[test]
#[should_panic]
fn governance_set_is_allocator_revoke_disallows_queue_ops() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let grantee = mk(12);
    c.set_is_allocator(grantee.clone(), true);

    let market_account = mk(9103);
    let cfg = MarketConfiguration {
        cap: U128(1),
        enabled: true,
        removable_at: TimestampNs::ZERO,
        cap_group_id: None,
    };

    let market_id = c.insert_market_for_tests(market_account, cfg, 0);

    // Revoke Allocator role; subsequent queue op by grantee should panic due to lack of rights
    c.set_is_allocator(grantee.clone(), false);
    set_ctx(
        &vault_id,
        &grantee,
        None,
        Some(yocto_for_bytes(storage_bytes_for_queue_account_id())),
    );
    c.set_supply_queue(vec![market_id]);
}

#[rstest(
    method_name,
    case("set_curator"),
    case("set_is_allocator"),
    case("set_skim_recipient"),
    case("set_fees"),
    case("set_restrictions"),
    case("submit_sentinel"),
    case("submit_timelock"),
    case("submit_cap"),
    case("submit_market_removal"),
    case("set_supply_queue")
)]
fn governance_abdicate_blocks_further_changes(method_name: &str) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);
        let owner = c.own_get_owner().unwrap();

        setup_env(&vault_id, &owner, vec![]);

        c.abdicate(method_name.to_string());
        match method_name {
            "set_curator" => {
                c.set_curator(mk(2));
            }
            "set_is_allocator" => {
                c.set_is_allocator(mk(4), false);
            }
            "submit_sentinel" => {
                c.submit_sentinel(mk(5));
                c.accept_sentinel();
                c.revoke_pending_sentinel();
            }
            "set_skim_recipient" => {
                c.set_skim_recipient(mk(1));
            }
            "set_fees" => {
                c.set_fees(build_fees(
                    Wad::one() / 10,
                    c.fees.management.fee,
                    accounts(1),
                    c.fees.management.recipient.clone(),
                ));
            }
            "set_restrictions" => {
                c.set_restrictions(Some(Restrictions::Paused));
                c.accept_restrictions();
                c.revoke_pending_restrictions();
            }
            "submit_timelock" => {
                let cur = c.get_configuration().initial_timelock_ns;
                // value choice irrelevant; abdication check runs first
                c.submit_timelock(cur, None);
                c.accept_timelock();
                c.revoke_pending_timelock();
            }
            "submit_cap" => {
                let market = mk(9200);
                c.submit_cap(market.clone(), U128(1));
                c.accept_cap(market.clone());
                c.revoke_pending_cap(market);
            }
            "submit_market_removal" => {
                let market = mk(9203);
                c.submit_market_removal(market.clone());
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
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    // First, ensure a sentinel is set by submitting and accepting once.
    let initial_sentinel = mk(1);
    c.submit_sentinel(initial_sentinel.clone());
    set_ctx(
        &vault_id,
        &owner,
        Some(env::block_timestamp() + 1_000_000_000),
        None,
    );
    c.accept_sentinel();

    let max_timelock = MAX_TIMELOCK_NS;
    c.governance_timelocks = Timelocks::new(
        TimestampNs(max_timelock),
        TimestampNs(max_timelock),
        TimestampNs(max_timelock),
        TimestampNs(max_timelock),
    );
    // Now submit another sentinel change but do not advance time.
    let new_sentinel = mk(5);
    set_ctx(&vault_id, &owner, None, None);
    c.submit_sentinel(new_sentinel);
    c.accept_sentinel();
}

#[test]
#[should_panic]
fn governance_submit_accept_and_revoke_guardian() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let new_sentinel = mk(4);
    c.submit_sentinel(new_sentinel.clone());

    set_ctx(
        &vault_id,
        &owner,
        Some(env::block_timestamp() + 1_000_000_000),
        None,
    );
    c.accept_sentinel();

    // Stage another pending and then revoke it
    let another = mk(3);
    set_ctx(&vault_id, &owner, None, None);
    c.submit_sentinel(another);
    c.revoke_pending_sentinel();

    c.accept_sentinel();
}

#[test]
fn governance_submit_accept_and_revoke_sentinel() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let new_sentinel = mk(4);
    c.submit_sentinel(new_sentinel.clone());

    let future = env::block_timestamp().saturating_add(1_000_000_000);
    set_ctx(&vault_id, &owner, Some(future), None);
    c.accept_sentinel();

    let cfg = c.get_configuration();
    assert_eq!(
        cfg.sentinel, new_sentinel,
        "Sentinel should update after accept"
    );

    // Stage another change and revoke it as the sentinel
    let another = mk(3);
    set_ctx(&vault_id, &owner, None, None);
    c.submit_sentinel(another);

    let sentinel = cfg.sentinel;
    set_ctx(&vault_id, &sentinel, None, None);
    c.revoke_pending_sentinel();

    set_ctx(&vault_id, &owner, None, None);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.accept_sentinel()));
    assert!(
        result.is_err(),
        "accept_sentinel should panic when nothing pending"
    );
}

#[test]
fn sentinel_can_revoke_pending_cap_change() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let market = mk(9020);
    let new_cap = U128(1);
    c.submit_cap(market.clone(), new_cap);

    assert_eq!(c.governance_timelocks.pending_len(), 1);

    let sentinel = c.get_configuration().sentinel;
    set_ctx(&vault_id, &sentinel, None, None);
    c.revoke_pending_cap(market.clone());

    assert!(
        c.governance_timelocks.pending_len() == 0,
        "Sentinel should be able to revoke pending caps"
    );
}

#[test]
fn governance_submit_accept_timelock_increase_then_decrease() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let cur = c.get_configuration().initial_timelock_ns;

    // Increase applies immediately
    c.submit_timelock((cur.0 + 1).into(), None);
    assert_eq!(
        c.get_configuration().initial_timelock_ns.0,
        cur.0 + 1,
        "timelock should increase immediately"
    );

    // Decrease schedules a pending change
    c.submit_timelock(cur, None);
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
#[should_panic = "No pending change"]
fn governance_accept_timelock_without_pending_panics() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    c.accept_timelock();
}

#[test]
#[should_panic = "No pending change"]
fn governance_revoke_pending_timelock_then_accept_panics() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let cur = c.get_configuration().initial_timelock_ns;

    // Force a pending by first increasing then decreasing
    c.submit_timelock((cur.0 + 1).into(), None);
    c.submit_timelock(cur, None);

    c.revoke_pending_timelock();
    c.accept_timelock();
}

#[test]
fn governance_submit_cap_immediate_decrease() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let m = mk(9104);
    let cfg = MarketConfiguration {
        cap: U128(100),
        enabled: true,
        removable_at: TimestampNs::ZERO,
        cap_group_id: None,
    };

    let _market_id = c.insert_market_for_tests(m.clone(), cfg, 0);

    c.submit_cap(m.clone(), U128(50));
    let cfg_after = must_market_record(&c, &m);
    assert_eq!(
        cfg_after.cfg.cap.0, 50,
        "cap decrease must apply immediately"
    );
}

#[test]
fn cap_group_membership_moves_principal() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    c.governance_timelocks = Timelocks::new(
        TimestampNs::ZERO,
        TimestampNs::ZERO,
        TimestampNs::ZERO,
        TimestampNs::ZERO,
    );

    let group_a = CapGroupId::try_from("ga".to_string()).unwrap();
    let group_b = CapGroupId::try_from("gb".to_string()).unwrap();

    c.submit_cap_group_update(CapGroupUpdate::SetCap {
        cap_group_id: group_a.clone(),
        new_cap: Some(200),
    });
    c.submit_cap_group_update(CapGroupUpdate::SetCap {
        cap_group_id: group_b.clone(),
        new_cap: Some(300),
    });

    let market = mk(9400);
    let cfg = MarketConfiguration {
        cap: U128(200),
        enabled: true,
        removable_at: TimestampNs::ZERO,
        cap_group_id: Some(group_a.clone()),
    };
    let market_id = c.insert_market_for_tests(market.clone(), cfg, 80);

    c.submit_cap_group_update(CapGroupUpdate::SetMembership {
        market_id: market_id.0,
        cap_group_id: Some(group_b.clone()),
    });
    c.accept_cap_group_update(CapGroupUpdateKey::SetMembership {
        market_id: market_id.0,
    });

    let rec = c.markets.get(&market_id).expect("market must exist");
    assert_eq!(rec.cfg.cap_group_id, Some(group_b.clone()));

    assert_eq!(
        c.cap_groups
            .get(&group_a)
            .expect("group a must exist")
            .principal,
        0
    );
    assert_eq!(
        c.cap_groups
            .get(&group_b)
            .expect("group b must exist")
            .principal,
        80
    );
}

#[test]
fn governance_cap_group_relative_cap_decrease_immediate_increase_timelocked() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    c.governance_timelocks = Timelocks::new(
        TimestampNs::ZERO,
        TimestampNs::ZERO,
        TimestampNs::ZERO,
        TimestampNs::ZERO,
    );

    let group = CapGroupId::try_from("gr".to_string()).unwrap();

    c.submit_cap_group_update(CapGroupUpdate::SetCap {
        cap_group_id: group.clone(),
        new_cap: Some(1_000),
    });

    assert_eq!(
        cap_group_relative_cap(c.cap_groups.get(&group).expect("group must exist")),
        Wad::one()
    );

    let half = Wad::one() / 2;
    c.submit_cap_group_update(CapGroupUpdate::SetRelativeCap {
        cap_group_id: group.clone(),
        new_relative_cap: Some(half),
    });

    assert_eq!(
        cap_group_relative_cap(c.cap_groups.get(&group).expect("group must exist")),
        half
    );
    assert!(
        c.governance_timelocks.pending_len() == 0,
        "decreasing relative cap should apply immediately"
    );

    c.submit_cap_group_update(CapGroupUpdate::SetRelativeCap {
        cap_group_id: group.clone(),
        new_relative_cap: Some(Wad::one()),
    });

    assert!(
        c.governance_timelocks.pending_len() > 0,
        "increasing relative cap should be timelocked"
    );
    assert_eq!(
        cap_group_relative_cap(c.cap_groups.get(&group).expect("group must exist")),
        half,
        "relative cap should not update until accepted"
    );

    c.accept_cap_group_update(CapGroupUpdateKey::SetRelativeCap {
        cap_group_id: group.clone(),
    });

    assert_eq!(
        cap_group_relative_cap(c.cap_groups.get(&group).expect("group must exist")),
        Wad::one(),
    );
}

#[test]
fn governance_submit_and_accept_cap_new_market_creates_and_enables() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let m = mk(9105);

    set_ctx(&vault_id, &owner, None, Some(yocto_for_bytes(20_000)));
    c.submit_cap(m.clone(), U128(5));

    set_ctx(
        &vault_id,
        &owner,
        Some(env::block_timestamp() + 1_000_000_000),
        None,
    );
    c.accept_cap(m.clone());

    let cfg = &must_market_record(&c, &m).cfg;
    assert_eq!(cfg.cap.0, 5);
    assert!(
        cfg.enabled,
        "market should be enabled after accepting raise"
    );
}

#[test]
#[should_panic = "No pending cap change for this market"]
fn governance_revoke_pending_cap_then_accept_panics() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let m = mk(9106);

    set_ctx(&vault_id, &owner, None, Some(yocto_for_bytes(20_000)));
    c.submit_cap(m.clone(), U128(7));

    set_ctx(&vault_id, &owner, None, None);
    c.revoke_pending_cap(m.clone());
    c.accept_cap(m);
}

#[test]
fn governance_submit_and_revoke_market_removal() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);
    let new_ts = env::block_timestamp() + 1_000_000_000;
    set_ctx(&vault_id, &owner, Some(new_ts), None);

    let m = mk(9107);
    let cfg = MarketConfiguration {
        cap: U128(0),
        enabled: true,
        removable_at: TimestampNs::ZERO,
        cap_group_id: None,
    };

    let _market_id = c.insert_market_for_tests(m.clone(), cfg, 1);

    c.submit_market_removal(m.clone());
    c.accept_market_removal(m.clone());
    let after = must_market_record(&c, &m);
    assert_eq!(
        after.cfg.removable_at,
        TimestampNs(new_ts),
        "removal must be scheduled"
    );

    c.revoke_pending_market_removal(m.clone());
    let after2 = must_market_record(&c, &m);
    assert_eq!(
        after2.cfg.removable_at,
        TimestampNs::ZERO,
        "removal must be revoked"
    );
}

#[test]
fn governance_set_skim_recipient_updates_field() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = mk(1);
    setup_env(&vault_id, &owner, vec![]);

    let new_recipient = mk(4);
    c.set_skim_recipient(new_recipient.clone());
    assert_eq!(c.skim_recipient, new_recipient);
}

#[test]
fn governance_set_fees_zero_fee_recipient_change_no_accrue() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = mk(1);

    owner_call_env(vault_id, &owner);

    c.deposit_unchecked(&owner, 1_000)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));
    c.idle_balance = 1_500;
    c.fee_anchor.total_assets = 1_000.into();
    c.fee_anchor.timestamp_ns = env::block_timestamp().into();
    c.fees.performance.fee = Wad::zero();
    c.fees.management.fee = Wad::zero();

    let ts_before = c.total_supply();
    let last_before = c.fee_anchor.total_assets;

    let new_recipient = mk(5);
    let old_recipient = c.fees.performance.recipient.clone();

    c.set_fees(build_fees(
        c.fees.performance.fee,
        c.fees.management.fee,
        new_recipient.clone(),
        new_recipient.clone(),
    ));

    assert_eq!(c.governance_timelocks.pending_len(), 1);
    assert_eq!(c.fees.performance.recipient, old_recipient);

    c.accept_fees();

    assert_eq!(
        c.total_supply(),
        ts_before,
        "no fee shares minted when fee=0"
    );
    assert_eq!(
        c.fee_anchor.total_assets, last_before,
        "fee anchor should not change when fee=0"
    );
    assert_eq!(c.fees.performance.recipient, new_recipient);
    assert_eq!(c.governance_timelocks.pending_len(), 0);
}

fn owner_call_env(vault_id: AccountId, owner: &AccountId) {
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
}

#[test]
#[should_panic = "Refusing to skim the underlying token"]
fn skim_rejects_underlying_token() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let recipient = mk(4);
    c.set_skim_recipient(recipient.clone());

    let underlying: AccountId = c.underlying_asset.contract_id().into();
    let _ = c.skim(underlying);
}

#[test]
#[should_panic = "Refusing to skim the share token"]
fn skim_rejects_share_token() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let recipient = mk(4);
    c.set_skim_recipient(recipient.clone());

    let share_token: AccountId = vault_id.clone();
    let _ = c.skim(share_token);
}

#[rstest]
fn after_supply_1_check_allocating_not_allocating(c_max: Contract) {
    let mut c = c_max;

    c.op_state = OpState::Idle;

    c.supply_01_handle_transfer(
        Ok(U128(1)),
        MarketId(1),
        0,
        2,
        Default::default(),
        Default::default(),
    )
    .detach();

    assert_eq!(c.op_state, OpState::Idle);
}

#[test]
fn after_supply_1_check_allocating_not_allocating_index() {
    let vault_id = mk(0);
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
        plan: Default::default(),
    });

    c.supply_01_handle_transfer(
        Ok(U128(1)),
        MarketId(1),
        op_id + 1,
        0,
        Default::default(),
        Default::default(),
    )
    .detach();

    assert_eq!(c.op_state, OpState::Idle);
}

#[test]
fn after_supply_1_check_allocating() {
    let vault_id = mk(0);
    setup_env(
        &vault_id,
        &vault_id,
        vec![PromiseResult::Successful(
            near_sdk::serde_json::to_vec(&U128(u128::MAX))
                .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string())),
        )],
    );

    let mut c = new_test_contract(&vault_id);

    let market_id = c.insert_market_for_tests(mk(3), MarketConfiguration::default(), 0);

    let op_id = 1;

    c.op_state = OpState::Allocating(AllocatingState {
        op_id,
        index: 0u32,
        remaining: 0u128,
        plan: Default::default(),
    });

    c.supply_01_handle_transfer(
        Ok(U128(1)),
        market_id,
        op_id,
        0,
        Default::default(),
        Default::default(),
    )
    .detach();

    assert_eq!(
        c.op_state,
        OpState::Allocating(AllocatingState {
            op_id,
            index: 0,
            plan: Default::default(),
            remaining: 0u128
        })
    );
}

#[rstest]
fn after_exec_withdraw_read_none_to_payout(
    #[with(vault_id(), vec![(mk(8), 0, true, 100, false)])] mut c: Contract,
) {
    let (market_id, record) = c.markets.clone().into_iter().next().unwrap();
    let _market_account = record.account.clone();
    let principal = 100u128;
    c.withdraw_route = vec![market_id].into();

    let op_id = 42;
    let index = 0;
    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id,
        request_id: op_id,
        index,
        remaining: 60,
        receiver: account_id_to_address(&mk(9)),
        collected: 10,
        owner: account_id_to_address(&mk(1)),
        escrow_shares: 50,
    });
    c.remember_account_mapping(account_id_to_address(&mk(1)), mk(1));
    c.remember_account_mapping(account_id_to_address(&mk(9)), mk(9));

    let res = c.execute_withdraw_02_reconcile_position(
        Ok(None),
        op_id,
        market_id,
        U64(0),
        U128(principal),
        U128(0),
    );
    match res {
        PromiseOrValue::Promise(_p) => {}
        _ => panic!("Expected a Promise to proceed to balance settlement"),
    }

    let res2 = c.execute_withdraw_03_settle(
        Ok(U128(principal)),
        op_id,
        market_id,
        U64(0),
        U128(principal),
        U128(0),
        U128(0),
    );

    match res2 {
        PromiseOrValue::Promise(_p) => {}
        _ => panic!("Expected a Promise to send payout after settlement"),
    }

    assert_eq!(
        c.markets.get(&market_id).map(|r| r.principal).unwrap(),
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
#[should_panic(expected = "No balance to skim")]
fn after_skim_balance_zero_noop() {
    let vault_id = mk(0);
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
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);

    let mut c = new_test_contract(&vault_id);

    let res = c.skim_01_read_balance(Ok(U128(123)), mk(10), mk(11));
    match res {
        PromiseOrValue::Promise(_) => { //NOTE: one day we will be able to read the promise
        }
        _ => panic!("Skim with positive balance must return a Promise"),
    }
}

#[rstest(
    before => [0u128, 1u128, 100u128],
    need => [0u128, 1u128, 50u128],
    collected => [1u128, 2u128]
)]
fn prop_after_exec_withdraw_read_err_no_change(before: u128, need: u128, collected: u128) {
    use templar_common::vault::PendingWithdrawal;

    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let market_account = mk(8);
    let market_id = c.insert_market_for_tests(
        market_account.clone(),
        MarketConfiguration::default(),
        before,
    );
    c.withdraw_route = vec![market_id].into();

    let initial_idle = c.idle_balance;

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 99,
        request_id: 99,
        index: 0,
        remaining: need,
        receiver: account_id_to_address(&mk(9)),
        collected,
        owner: account_id_to_address(&mk(1)),
        escrow_shares: 0,
    });

    c.insert_pending_withdrawal_for_tests(
        0,
        PendingWithdrawal {
            receiver: mk(9),
            owner: mk(1),
            escrow_shares: 0,
            expected_assets: collected,
            requested_at: 0,
        },
    );

    let res = c.execute_withdraw_02_reconcile_position(
        Err(near_sdk::PromiseError::Failed),
        99,
        market_id,
        U64(0),
        U128(before),
        U128(0),
    );
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) due to read failure and stop"),
    }

    assert_eq!(
        c.markets.get(&market_id).map_or(u128::MAX, |r| r.principal),
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
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let market_account = mk(8);
    let market_id = c.insert_market_for_tests(market_account, MarketConfiguration::default(), 10);
    c.withdraw_route = vec![market_id].into();

    let real_op = 5u64;
    let real_idx = 0u32;

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: real_op,
        request_id: real_op,
        index: real_idx,
        remaining: 1,
        receiver: account_id_to_address(&mk(9)),
        collected: 1,
        owner: account_id_to_address(&mk(1)),
        escrow_shares: 0,
    });

    c.insert_pending_withdrawal_for_tests(
        real_idx as u64,
        PendingWithdrawal {
            receiver: mk(9),
            owner: mk(1),
            escrow_shares: 0,
            expected_assets: 1,
            requested_at: 0,
        },
    );

    let call_op = if pass_op { real_op } else { real_op + 1 };
    let call_market = if pass_index {
        market_id
    } else {
        MarketId(market_id.0.saturating_add(1))
    };

    let r = c.execute_withdraw_02_reconcile_position(
        Ok(None),
        call_op,
        call_market,
        U64(0),
        U128(10),
        U128(0),
    );
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

#[rstest]
fn refund_path_consistency(#[with(vault_id(), vec![(mk(8), 0, true, 10, false)])] mut c: Contract) {
    use near_sdk_contract_tools::ft::Nep141Controller as _;

    let market_account = mk(8);
    let market_id = must_market_id(&c, &market_account);
    c.withdraw_route = vec![market_id].into();
    let owner = mk(1);
    c.deposit_unchecked(&near_sdk::env::current_account_id(), 10)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));

    let op_id = 77;
    let index = 0;
    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id,
        request_id: op_id,
        index,
        remaining: 0,
        receiver: account_id_to_address(&mk(9)),
        collected: 0,
        owner: account_id_to_address(&owner),
        escrow_shares: 10,
    });
    c.insert_pending_withdrawal_for_tests(
        c.queue_tail(),
        PendingWithdrawal {
            owner: owner.clone(),
            receiver: mk(9),
            escrow_shares: 10,
            expected_assets: 0,
            requested_at: 0,
        },
    );

    let supply_before = c.total_supply();
    let vault_before = c.balance_of(&near_sdk::env::current_account_id());
    let owner_before = c.balance_of(&owner);

    // Read result with need=0 ensures credited=0; triggers refund branch
    let res = c.execute_withdraw_02_reconcile_position(
        Ok(None),
        op_id,
        market_id,
        U64(0),
        U128(0),
        U128(0),
    );
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise to proceed to balance settlement"),
    }

    let res2 = c.execute_withdraw_03_settle(
        Ok(U128(0)), // no inflow observed
        op_id,
        market_id,
        U64(0),
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
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    c.op_state = OpState::Allocating(AllocatingState {
        op_id: 42,
        index: 3,
        remaining: 77,
        plan: Default::default(),
    });

    let ctx = c.ctx_allocating(42).expect("ctx_allocating should succeed");
    assert_eq!(
        ctx,
        &AllocatingState {
            op_id: 42,
            index: 3,
            remaining: 77,
            plan: Default::default(),
        }
    );

    // Wrong op_id => error
    assert!(c.ctx_allocating(43).is_err());
}

#[test]
fn ctx_withdrawing_ok_and_err() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    let recv = mk(1);
    let owner = mk(1);

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 7,
        request_id: 7,
        index: 1,
        remaining: 50,
        receiver: account_id_to_address(&recv),
        collected: 5,
        owner: account_id_to_address(&owner),
        escrow_shares: 10,
    });

    let ctx = c
        .ctx_withdrawing(7)
        .expect("ctx_withdrawing should succeed");
    assert_eq!(ctx.index, 1);
    assert_eq!(ctx.remaining, 50);
    assert_eq!(ctx.receiver, account_id_to_address(&recv));
    assert_eq!(ctx.collected, 5);
    assert_eq!(ctx.owner, account_id_to_address(&owner));
    assert_eq!(ctx.escrow_shares, 10);

    // Wrong op_id => error
    assert!(c.ctx_withdrawing(8).is_err());
}

#[test]
fn resolve_market_helpers_supply_and_withdraw() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Withdraw resolver uses withdraw_route only
    let m1 = MarketId(1001);
    let m2 = MarketId(1002);
    c.withdraw_route = vec![m1, m2].into();
    assert_eq!(c.withdraw_route.first().copied(), Some(m1));
    assert_eq!(c.withdraw_route.get(1).copied(), Some(m2));
    assert_eq!(c.withdraw_route.get(2).copied(), None);
}

#[test]
fn after_supply_2_read_missing_position_stops() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Resolve market via supply_queue
    let market = MarketId(42);
    c.supply_queue.push(market);

    // Must be in Allocating ctx
    c.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 10,
        plan: Default::default(),
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
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Resolve market via supply_queue
    let market = MarketId(43);
    c.supply_queue.push(market);

    // Must be in Allocating ctx
    c.op_state = OpState::Allocating(AllocatingState {
        op_id: 7,
        index: 0,
        remaining: 100,
        plan: Default::default(),
    });

    // Read failure -> stop_and_exit
    let res = c.supply_02_position_read(
        Err(near_sdk::PromiseError::Failed),
        market,
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
    #[with(vault_id(), vec![(mk(50), 0, true, 100, false)])] mut c: Contract,
    receiver: AccountId,
    owner: AccountId,
) {
    let market_account = mk(50);
    let market_id = must_market_id(&c, &market_account);
    c.withdraw_route = vec![market_id].into();

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 21,
        request_id: 21,
        index: 0,
        remaining: 60,
        receiver: account_id_to_address(&receiver),
        collected: 10,
        owner: account_id_to_address(&owner),
        escrow_shares: 5,
    });

    let res = c.withdraw_01_handle_create_request(Ok(()), 21, market_id, U128(60));
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) when create succeeds and execution is deferred"),
    }
    // State remains Withdrawing; keeper must call execute_next_market_withdrawal
    assert!(matches!(c.op_state, OpState::Withdrawing { .. }));
}

#[test]
#[should_panic(expected = "Couldnt create withdraw request in market")]
fn rebalance_create_failure_keeps_idle_and_unlocks() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    assert!(matches!(c.op_state, OpState::Idle));
    assert!(
        !c.has_pending_market_withdrawal(),
        "no locks should be held initially",
    );

    let market = MarketId(51);
    let amount = U128(100);

    c.rebalance_withdraw_01_after_create_request(
        Err(near_sdk::PromiseError::Failed),
        market,
        amount,
    );

    assert!(
        matches!(c.op_state, OpState::Idle),
        "vault should remain Idle after rebalance create failure",
    );
    assert!(
        !c.has_pending_market_withdrawal(),
        "market execution lock must not be held after failure",
    );
}

#[rstest]
fn after_exec_withdraw_req_returns_promise(
    #[with(vault_id(), vec![(mk(60), 0, true, 10, false)])] mut c: Contract,
) {
    let market_account = mk(60);
    let market_id = must_market_id(&c, &market_account);
    c.withdraw_route = vec![market_id].into();

    let op_id = 33;
    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id,
        request_id: op_id,
        index: 0,
        remaining: 5,
        receiver: account_id_to_address(&mk(9)),
        collected: 0,
        owner: account_id_to_address(&mk(1)),
        escrow_shares: 0,
    });

    let res = c.execute_withdraw_01_execute_withdraw_fetch_position(
        Ok(U128(1)),
        op_id,
        market_id,
        U64(0),
        None,
    );
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
fn after_exec_withdraw_read_instant_payout_when_remaining_0(
    #[with(vault_id(), vec![(mk(70), 0, true, 10, false), (mk(71), 0, true, 0, false)])]
    mut c: Contract,
    owner: AccountId,
    receiver: AccountId,
) {
    let m1_account = mk(70);
    let m2_account = mk(71);
    let m1 = must_market_id(&c, &m1_account);
    let m2 = must_market_id(&c, &m2_account);
    c.withdraw_route = vec![m1, m2].into();
    let record_principal = 10u128;

    let op_id = 0;
    let index = 0;
    let before_balance = 0;

    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id,
        request_id: op_id,
        index,
        remaining: 10,
        receiver: account_id_to_address(&receiver),
        collected: 0,
        owner: account_id_to_address(&owner),
        escrow_shares: 0,
    });
    c.remember_account_mapping(account_id_to_address(&owner), owner.clone());
    c.remember_account_mapping(account_id_to_address(&receiver), receiver.clone());

    let res = c.execute_withdraw_02_reconcile_position(
        Ok(None),
        op_id,
        m1,
        U64(0),
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
    // We now queue up for execution
    c.execute_withdraw_03_settle(
        Ok(U128(record_principal)), // after_balance
        op_id,
        m1,
        U64(0),
        U128(record_principal), // before_principal
        U128(0),
        U128(before_balance),
    )
    .detach();

    match &c.op_state {
        OpState::Payout(PayoutState {
            op_id,
            request_id: _,
            receiver: r,
            amount,
            owner: o,
            escrow_shares,
            burn_shares,
        }) => {
            assert_eq!(*op_id, 0);
            assert_eq!(*amount, before_balance + record_principal);
            assert_eq!(*escrow_shares, 0);
            assert_eq!(*burn_shares, 0);
            assert_eq!(*r, account_id_to_address(&receiver));
            assert_eq!(*o, account_id_to_address(&owner));
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
    let amount = 77;

    // Seed escrowed shares into the vault's own account
    c.deposit_unchecked(&near_sdk::env::current_account_id(), escrow)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));

    c.insert_pending_withdrawal_for_tests(
        c.queue_tail(),
        PendingWithdrawal {
            owner: owner.clone(),
            receiver: receiver.clone(),
            escrow_shares: escrow,
            expected_assets: amount,
            requested_at: 0,
        },
    );

    // Enter Payout with non-zero escrow
    c.op_state = OpState::Payout(PayoutState {
        op_id: 123,
        request_id: 123,
        receiver: account_id_to_address(&receiver),
        amount,
        owner: account_id_to_address(&owner),
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
fn stop_and_exit_payout_reconcile_ignores_mismatched_op_id(
    mut c: Contract,
    owner: AccountId,
    receiver: AccountId,
) {
    use near_sdk_contract_tools::ft::Nep141Controller as _;

    let escrow: u128 = 10;
    let amount = 77;

    // Seed escrowed shares into the vault's own account so a wrong reconcile would refund them.
    c.deposit_unchecked(&near_sdk::env::current_account_id(), escrow)
        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));

    // Simulate that op_id=2 is the *current* payout.
    let head: u64 = 1;
    c.withdraw_queue.next_withdraw_to_execute = head;
    c.insert_pending_withdrawal_for_tests(
        head,
        PendingWithdrawal {
            owner: owner.clone(),
            receiver: receiver.clone(),
            escrow_shares: escrow,
            expected_assets: amount,
            requested_at: 0,
        },
    );
    c.insert_pending_withdrawal_for_tests(
        head.saturating_add(1),
        PendingWithdrawal {
            owner: owner.clone(),
            receiver: receiver.clone(),
            escrow_shares: 0,
            expected_assets: 1,
            requested_at: 0,
        },
    );

    let market = MarketId(999);
    c.withdraw_route = vec![market].into();
    c.market_execution_lock.lock(market, 2, u64::MAX / 2);

    c.idle_balance = 123;
    c.op_state = OpState::Payout(PayoutState {
        op_id: 2,
        request_id: 2,
        receiver: account_id_to_address(&receiver),
        amount,
        owner: account_id_to_address(&owner),
        escrow_shares: escrow,
        burn_shares: 0,
    });

    let supply_before = c.total_supply();
    let vault_before = c.balance_of(&near_sdk::env::current_account_id());
    let owner_before = c.balance_of(&owner);
    let idle_before = c.idle_balance;
    let head_before = c.withdraw_queue.next_withdraw_to_execute;
    let len_before = c.pending_withdrawals_len();
    let route_before = c.withdraw_route.clone();
    let locked_before = c.market_execution_lock.is_locked(market);

    // Simulate a late callback from a previous payout op_id=1.
    let res = c.stop_and_exit_payout_01_reconcile(Ok(U128(999)), 1, None);
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) from reconcile"),
    }

    assert!(matches!(
        c.op_state,
        OpState::Payout(PayoutState { op_id: 2, .. })
    ));
    assert_eq!(c.total_supply(), supply_before, "supply must not change");
    assert_eq!(
        c.balance_of(&near_sdk::env::current_account_id()),
        vault_before,
        "vault balance must not change"
    );
    assert_eq!(
        c.balance_of(&owner),
        owner_before,
        "owner must not be refunded"
    );
    assert_eq!(c.idle_balance, idle_before, "idle_balance must not resync");
    assert_eq!(
        c.withdraw_queue.next_withdraw_to_execute, head_before,
        "queue head must not advance"
    );
    assert_eq!(
        c.pending_withdrawals_len(),
        len_before,
        "queue must not dequeue"
    );
    assert_eq!(
        c.withdraw_route, route_before,
        "withdraw route must not clear"
    );
    assert_eq!(
        c.market_execution_lock.is_locked(market),
        locked_before,
        "market lock must not clear"
    );
}

#[rstest]
fn stop_and_exit_payout_zero_escrow_just_idle(
    mut c: Contract,
    owner: AccountId,
    receiver: AccountId,
) {
    // Enter Payout with zero escrow; no transfers should occur
    let amount = 1;
    c.op_state = OpState::Payout(PayoutState {
        op_id: 7,
        request_id: 7,
        receiver: account_id_to_address(&receiver),
        amount,
        owner: account_id_to_address(&owner),
        escrow_shares: 0,
        burn_shares: 0,
    });

    c.insert_pending_withdrawal_for_tests(
        c.queue_tail(),
        PendingWithdrawal {
            owner: owner.clone(),
            receiver: receiver.clone(),
            escrow_shares: 0,
            expected_assets: amount,
            requested_at: 0,
        },
    );

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
fn unbrick_withdrawing_refunds_and_dequeues() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    // Seed escrowed shares into the vault's own account
    let escrow: u128 = 10;
    c.deposit_unchecked(&near_sdk::env::current_account_id(), escrow)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));

    // Enqueue a pending withdrawal at the head
    let id_before = c.queue_tail();
    let receiver = mk(9);
    c.insert_pending_withdrawal_for_tests(
        id_before,
        PendingWithdrawal {
            owner: owner.clone(),
            receiver: receiver.clone(),
            escrow_shares: escrow,
            expected_assets: 1,
            requested_at: 0,
        },
    );

    // Simulate an in-flight withdrawing state
    c.withdraw_route = vec![MarketId(1001)].into();
    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 42,
        request_id: 42,
        index: 0,
        remaining: 1,
        receiver: account_id_to_address(&receiver),
        collected: 0,
        owner: account_id_to_address(&owner),
        escrow_shares: escrow,
    });

    let supply_before = c.total_supply();
    let vault_before = c.balance_of(&near_sdk::env::current_account_id());
    let owner_before = c.balance_of(&owner);
    let len_before = c.pending_withdrawals_len();

    let res = c.unbrick();
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) from unbrick"),
    }

    // Escrow refunded, head advanced, state reset
    assert!(
        matches!(c.op_state, OpState::Idle),
        "vault should return to Idle"
    );
    assert!(
        c.withdraw_route.is_empty(),
        "withdraw route must be cleared"
    );
    assert_eq!(c.total_supply(), supply_before, "no supply change");
    assert_eq!(
        c.balance_of(&near_sdk::env::current_account_id()),
        vault_before.saturating_sub(escrow),
        "vault should refund escrow to owner"
    );
    assert_eq!(
        c.balance_of(&owner),
        owner_before.saturating_add(escrow),
        "owner should receive escrow refund"
    );
    assert_eq!(
        c.pending_withdrawals_len(),
        len_before.saturating_sub(1),
        "queue should dequeue the in-flight request"
    );
    assert_eq!(
        c.withdraw_queue.next_withdraw_to_execute,
        id_before.saturating_add(1),
        "head should advance by one"
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
fn unbrick_noop_when_not_withdrawing() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    c.op_state = OpState::Idle;

    // Capture baseline
    let len_before = c.pending_withdrawals_len();
    let head_before = c.withdraw_queue.next_withdraw_to_execute;
    let supply_before = c.total_supply();
    let vault_before = c.balance_of(&near_sdk::env::current_account_id());
    let owner_before = c.balance_of(&owner);

    let res = c.unbrick();
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) from unbrick"),
    }

    // No changes expected when not Withdrawing
    assert!(matches!(c.op_state, OpState::Idle));
    assert_eq!(c.pending_withdrawals_len(), len_before);
    assert_eq!(c.withdraw_queue.next_withdraw_to_execute, head_before);
    assert_eq!(c.total_supply(), supply_before);
    assert_eq!(
        c.balance_of(&near_sdk::env::current_account_id()),
        vault_before
    );
    assert_eq!(c.balance_of(&owner), owner_before);
}

#[test]
fn unbrick_payout_reaches_recovery_path() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    let receiver = mk(29);
    setup_env(&vault_id, &owner, vec![]);

    let escrow: u128 = 10;
    c.deposit_unchecked(&near_sdk::env::current_account_id(), escrow)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));

    let head_before = c.queue_tail();
    c.insert_pending_withdrawal_for_tests(
        head_before,
        PendingWithdrawal {
            owner: owner.clone(),
            receiver: receiver.clone(),
            escrow_shares: escrow,
            expected_assets: 1,
            requested_at: 0,
        },
    );

    c.op_state = OpState::Payout(PayoutState {
        op_id: 88,
        request_id: 88,
        receiver: account_id_to_address(&receiver),
        amount: 1,
        owner: account_id_to_address(&owner),
        escrow_shares: escrow,
        burn_shares: escrow,
    });

    let len_before = c.pending_withdrawals_len();
    let vault_before = c.balance_of(&near_sdk::env::current_account_id());
    let owner_before = c.balance_of(&owner);
    let supply_before = c.total_supply();

    let res = c.unbrick();
    match res {
        PromiseOrValue::Promise(_) => {}
        _ => panic!("Expected Promise(_) from payout unbrick"),
    }

    assert!(matches!(
        c.op_state,
        OpState::Payout(PayoutState { op_id: 88, .. })
    ));
    assert_eq!(c.pending_withdrawals_len(), len_before);
    assert_eq!(
        c.balance_of(&near_sdk::env::current_account_id()),
        vault_before
    );
    assert_eq!(c.balance_of(&owner), owner_before);
    assert_eq!(c.total_supply(), supply_before);
    assert_eq!(c.withdraw_queue.next_withdraw_to_execute, head_before);
}

#[test]
fn sentinel_can_unbrick_withdrawing_state() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    let escrow: u128 = 10;
    c.deposit_unchecked(&near_sdk::env::current_account_id(), escrow)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));

    let id_before = c.queue_tail();
    let receiver = mk(19);
    c.insert_pending_withdrawal_for_tests(
        id_before,
        PendingWithdrawal {
            owner: owner.clone(),
            receiver: receiver.clone(),
            escrow_shares: escrow,
            expected_assets: 1,
            requested_at: 0,
        },
    );

    c.withdraw_route = vec![MarketId(1901)].into();
    c.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 77,
        request_id: 77,
        index: 0,
        remaining: 1,
        receiver: account_id_to_address(&receiver),
        collected: 0,
        owner: account_id_to_address(&owner),
        escrow_shares: escrow,
    });

    let len_before = c.pending_withdrawals_len();
    let sentinel = c.get_configuration().sentinel;
    setup_env(&vault_id, &sentinel, vec![]);

    let res = c.unbrick();
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) from sentinel unbrick"),
    }

    assert!(matches!(c.op_state, OpState::Idle));
    assert!(c.withdraw_route.is_empty());
    assert_eq!(
        c.pending_withdrawals_len(),
        len_before.saturating_sub(1),
        "Sentinel unbrick should dequeue head"
    );
}

#[rstest(
    idle, amount, remaining, collected,
    case(0u128, 0u128, 0u128, 0u128),
    case(100u128, 0u128, 0u128, 0u128),
    case(0u128, 50u128, 50u128, 0u128),
    case(100u128, 50u128, 0u128, 50u128),
    case(100u128, 100u128, 0u128, 100u128),
    case(100u128, 150u128, 50u128, 100u128),
    case(u128::MAX, 1u128, 0u128, 1u128),
    case(1u128, u128::MAX, u128::MAX - 1u128, 1u128),
)]
fn compute_idle_coverage_cases(
    mut c: Contract,
    idle: u128,
    amount: u128,
    remaining: u128,
    collected: u128,
) {
    c.idle_balance = idle;
    let idle_before = c.idle_balance;

    let cov = c.compute_idle_coverage(amount);

    assert_eq!(
        cov.remaining_unmet, remaining,
        "remaining should match expected"
    );
    assert_eq!(
        cov.collected_from_idle, collected,
        "collected should match expected"
    );
    assert_eq!(
        cov.remaining_unmet.saturating_add(cov.collected_from_idle),
        amount,
        "invariant: remaining + collected == amount"
    );
    assert_eq!(
        c.idle_balance, idle_before,
        "compute_idle_coverage must not mutate idle_balance"
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

#[test]
fn peek_next_pending_withdrawal_id_empty_returns_none() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let c = new_test_contract(&vault_id);

    assert_eq!(c.pending_withdrawals_len(), 0, "queue should start empty");

    let head_before = c.withdraw_queue.next_withdraw_to_execute;
    let tail_before = c.queue_tail();
    assert_eq!(
        head_before, tail_before,
        "empty queue invariant: head == tail"
    );

    let got = c.peek_next_pending_withdrawal_id();
    assert!(got.is_none(), "expected None for empty queue");

    // Subsequent call still None and state unchanged
    let got2 = c.peek_next_pending_withdrawal_id();
    assert!(got2.is_none(), "expected None on repeated peek");
    assert_eq!(
        c.withdraw_queue.next_withdraw_to_execute, head_before,
        "peek must not mutate the head"
    );
    assert_eq!(
        c.pending_withdrawals_len(),
        0,
        "peek must not change the queue length"
    );
}

#[test]
fn peek_next_pending_withdrawal_id_nonempty_returns_head_and_does_not_mutate() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let mut c = new_test_contract(&vault_id);

    // Enqueue two pending withdrawals at tail positions
    let head_before = c.withdraw_queue.next_withdraw_to_execute;

    let id1 = c.queue_tail();
    c.insert_pending_withdrawal_for_tests(
        id1,
        PendingWithdrawal {
            owner: mk(1),
            receiver: mk(9),
            escrow_shares: 1,
            expected_assets: 1,
            requested_at: 0,
        },
    );

    let id2 = c.queue_tail();
    c.insert_pending_withdrawal_for_tests(
        id2,
        PendingWithdrawal {
            owner: mk(2),
            receiver: mk(10),
            escrow_shares: 2,
            expected_assets: 2,
            requested_at: 0,
        },
    );

    assert!(
        head_before < c.queue_tail(),
        "sanity: queue should now be non-empty (head < tail)"
    );

    // Peek should return the current head id
    let got = c.peek_next_pending_withdrawal_id();
    assert_eq!(
        got,
        Some(head_before),
        "peek should return the current head id"
    );

    // Ensure peek does not mutate any state
    assert_eq!(
        c.withdraw_queue.next_withdraw_to_execute, head_before,
        "head must be unchanged by peek"
    );
    assert_eq!(
        c.pending_withdrawals_len(),
        2,
        "peek must not modify queue length"
    );

    // Repeated peek yields the same result
    let got2 = c.peek_next_pending_withdrawal_id();
    assert_eq!(got2, Some(head_before));
}

#[test]
fn migrate_pending_withdrawals_preserves_fifo_and_tail() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let c = new_test_contract(&vault_id);

    let mut pending = IterableMap::new(StorageKey::PendingWithdrawals);
    let owner_a = mk(11);
    let receiver_a = mk(12);
    let owner_b = mk(13);
    let receiver_b = mk(14);

    pending.insert(
        5,
        PendingWithdrawal {
            owner: owner_a.clone(),
            receiver: receiver_a.clone(),
            escrow_shares: 10,
            expected_assets: 100,
            requested_at: 777,
        },
    );
    pending.insert(
        8,
        PendingWithdrawal {
            owner: owner_b.clone(),
            receiver: receiver_b.clone(),
            escrow_shares: 20,
            expected_assets: 200,
            requested_at: 888,
        },
    );
    pending.flush();

    let old = OldContract {
        underlying_asset: c.underlying_asset,
        aum: c.aum,
        fees: c.fees,
        skim_recipient: c.skim_recipient,
        fee_anchor: c.fee_anchor,
        idle_balance: c.idle_balance,
        op_state: c.op_state,
        next_op_id: c.next_op_id,
        last_refresh_ns: c.last_refresh_ns,
        refresh_cooldown_ns: c.refresh_cooldown_ns,
        idle_resync_last_ns: c.idle_resync_last_ns,
        idle_resync_cooldown_ns: c.idle_resync_cooldown_ns,
        idle_resync_inflight_op_id: c.idle_resync_inflight_op_id,
        virtual_shares: c.virtual_shares,
        virtual_assets: c.virtual_assets,
        markets: c.markets,
        market_ids: c.market_ids,
        cap_groups: c.cap_groups,
        next_market_id: c.next_market_id,
        governance_timelocks: c.governance_timelocks,
        supply_queue: c.supply_queue.clone().into(),
        pending_withdrawals: pending,
        next_withdraw_to_execute: 5,
        market_execution_lock: templar_common::vault::Locker::default(),
        withdraw_route: c.withdraw_route.clone().into(),
        abdicator: c.abdicator,
        gate: c.gate,
    };

    env::state_write(&old);
    let migrated = Contract::migrate();

    assert_eq!(migrated.pending_withdrawals_len(), 2);
    assert_eq!(migrated.withdraw_queue.next_withdraw_to_execute, 5);
    assert_eq!(migrated.withdraw_queue.next_pending_withdrawal_id, 9);

    let (head_id, head) = migrated.withdraw_queue.head().expect("head exists");
    assert_eq!(head_id, 5);
    assert_eq!(migrated.resolve_account(&head.owner), owner_a);
    assert_eq!(migrated.resolve_account(&head.receiver), receiver_a);

    let tail = migrated
        .withdraw_queue
        .pending_withdrawals()
        .get(&8)
        .unwrap();
    assert_eq!(migrated.resolve_account(&tail.owner), owner_b);
    assert_eq!(migrated.resolve_account(&tail.receiver), receiver_b);
}

#[test]
fn migrate_empty_queue_sets_tail_to_head() {
    let vault_id = mk(0);
    setup_env(&vault_id, &vault_id, vec![]);
    let c = new_test_contract(&vault_id);

    let mut pending = IterableMap::new(StorageKey::PendingWithdrawals);
    pending.flush();
    let old = OldContract {
        underlying_asset: c.underlying_asset,
        aum: c.aum,
        fees: c.fees,
        skim_recipient: c.skim_recipient,
        fee_anchor: c.fee_anchor,
        idle_balance: c.idle_balance,
        op_state: c.op_state,
        next_op_id: c.next_op_id,
        last_refresh_ns: c.last_refresh_ns,
        refresh_cooldown_ns: c.refresh_cooldown_ns,
        idle_resync_last_ns: c.idle_resync_last_ns,
        idle_resync_cooldown_ns: c.idle_resync_cooldown_ns,
        idle_resync_inflight_op_id: c.idle_resync_inflight_op_id,
        virtual_shares: c.virtual_shares,
        virtual_assets: c.virtual_assets,
        markets: c.markets,
        market_ids: c.market_ids,
        cap_groups: c.cap_groups,
        next_market_id: c.next_market_id,
        governance_timelocks: c.governance_timelocks,
        supply_queue: c.supply_queue.clone().into(),
        pending_withdrawals: pending,
        next_withdraw_to_execute: 7,
        market_execution_lock: templar_common::vault::Locker::default(),
        withdraw_route: c.withdraw_route.clone().into(),
        abdicator: c.abdicator,
        gate: c.gate,
    };

    env::state_write(&old);
    let migrated = Contract::migrate();

    assert_eq!(migrated.pending_withdrawals_len(), 0);
    assert_eq!(migrated.withdraw_queue.next_withdraw_to_execute, 7);
    assert_eq!(migrated.withdraw_queue.next_pending_withdrawal_id, 7);
    assert!(migrated.withdraw_queue.head().is_none());
}

#[test]
fn execute_withdrawal_empty_queue_noop() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    assert!(matches!(c.op_state, OpState::Idle));
    assert_eq!(c.pending_withdrawals_len(), 0, "queue should be empty");

    let res = c.execute_withdrawal(vec![]);
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) when queue is empty"),
    }

    assert!(
        matches!(c.op_state, OpState::Idle),
        "state must remain Idle"
    );
    assert_eq!(
        c.get_current_withdraw_request_id(),
        None,
        "no current request id when idle"
    );
    assert!(c.withdraw_route.is_empty(), "route must remain empty");
}

#[test]
fn execute_withdrawal_accrues_fee_shares() {
    let vault_id = accounts(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);

    c.deposit_unchecked(&owner, 1_000)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));
    c.fees.performance.fee = Wad::one() / 10;
    c.idle_balance += 250;

    let fee_recipient = c.fees.performance.recipient.clone();
    let balance_before = c.balance_of(&fee_recipient);

    let res = c.execute_withdrawal(vec![]);
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) when queue is empty"),
    }

    let balance_after = c.balance_of(&fee_recipient);
    assert!(
        balance_after > balance_before,
        "execute_withdrawal should mint pending performance fees",
    );
    assert_eq!(
        c.get_last_total_assets(),
        c.get_total_assets(),
        "last_total_assets should sync with current assets after accrual",
    );
}

#[rstest]
fn execute_withdrawal_skips_dust_and_starts_withdraw(
    #[with(vault_id(), vec![(mk(1234), 0, true, 50, false)])] mut c: Contract,
) {
    let owner_id = c.own_get_owner().unwrap();
    setup_env(&near_sdk::env::current_account_id(), &owner_id, vec![]);
    c.withdrawal_cooldown_ns = 0;

    // Prepare a withdraw market so a non-empty route makes sense
    let market_account = mk(1234);
    let market_id = must_market_id(&c, &market_account);

    // Enqueue a dust head (expected_assets = 0)
    let head_before = c.queue_tail();
    c.insert_pending_withdrawal_for_tests(
        head_before,
        PendingWithdrawal {
            owner: owner_id.clone(),
            receiver: mk(9),
            escrow_shares: 1,
            expected_assets: 0,
            requested_at: 0,
        },
    );

    // Followed by a real pending withdrawal
    let receiver = mk(10);
    let escrow: u128 = 5;
    let expected: u128 = 60;
    let id1 = c.queue_tail();
    c.insert_pending_withdrawal_for_tests(
        id1,
        PendingWithdrawal {
            owner: owner_id.clone(),
            receiver: receiver.clone(),
            escrow_shares: escrow,
            expected_assets: expected,
            requested_at: 0,
        },
    );

    c.idle_balance = 0; // force route-based execution

    let res = c.execute_withdrawal(vec![market_id]);
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) to signal offchain to execute next market"),
    }

    // Dust head must be removed and head advanced to the second request
    assert_eq!(
        c.withdraw_queue.next_withdraw_to_execute, id1,
        "head should advance past dust"
    );
    assert_eq!(
        c.pending_withdrawals_len(),
        1,
        "one item should remain in queue"
    );
    assert_eq!(
        c.get_current_withdraw_request_id(),
        Some(near_sdk::json_types::U64(id1)),
        "current request should be the second item"
    );
    assert_eq!(
        c.withdraw_route,
        vec![market_id].into(),
        "route must be set from input"
    );

    match &c.op_state {
        OpState::Withdrawing(s) => {
            assert_eq!(s.index, 0);
            assert_eq!(
                s.remaining, expected,
                "no idle used so remaining equals expected"
            );
            assert_eq!(s.collected, 0, "no idle collected");
            assert_eq!(s.owner, account_id_to_address(&owner_id));
            assert_eq!(s.receiver, account_id_to_address(&receiver));
            assert_eq!(s.escrow_shares, escrow);
        }
        other => panic!("Expected Withdrawing state, got {:?}", other),
    }
}

#[test]
fn execute_withdrawal_only_dust_drains_queue() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    setup_env(&vault_id, &owner, vec![]);
    c.withdrawal_cooldown_ns = 0;

    // Two dust entries
    let id0 = c.queue_tail();
    c.insert_pending_withdrawal_for_tests(
        id0,
        PendingWithdrawal {
            owner: owner.clone(),
            receiver: mk(9),
            escrow_shares: 1,
            expected_assets: 0,
            requested_at: 0,
        },
    );
    let id1 = c.queue_tail();
    c.insert_pending_withdrawal_for_tests(
        id1,
        PendingWithdrawal {
            owner,
            receiver: mk(10),
            escrow_shares: 2,
            expected_assets: 0,
            requested_at: 0,
        },
    );

    let res = c.execute_withdrawal(vec![]);
    match res {
        PromiseOrValue::Value(()) => {}
        _ => panic!("Expected Value(()) after draining dust-only queue"),
    }

    assert!(matches!(c.op_state, OpState::Idle), "must remain Idle");
    assert_eq!(c.pending_withdrawals_len(), 0, "queue should be empty");
    assert_eq!(
        c.withdraw_queue.next_withdraw_to_execute,
        id1.saturating_add(1),
        "head should advance by two"
    );
    assert!(c.withdraw_route.is_empty(), "route must remain empty");
}

#[test]
fn address_book_prunes_completed_withdrawal_addresses() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    let receiver = mk(9);
    setup_env(&vault_id, &owner, vec![]);

    let id = c.queue_tail();
    c.insert_pending_withdrawal_for_tests(
        id,
        PendingWithdrawal {
            owner: owner.clone(),
            receiver: receiver.clone(),
            escrow_shares: 1,
            expected_assets: 0,
            requested_at: 0,
        },
    );

    assert_eq!(c.resolve_account(&account_id_to_address(&owner)), owner);
    assert_eq!(
        c.resolve_account(&account_id_to_address(&receiver)),
        receiver
    );

    c.pop_head();
    assert_eq!(c.pending_withdrawals_len(), 0, "queue should be empty");
    assert!(!c.address_book.contains_key(&account_id_to_address(&owner)));
    assert!(!c
        .address_book
        .contains_key(&account_id_to_address(&receiver)));
}

#[test]
fn address_book_keeps_live_withdrawing_addresses_off_queue() {
    let vault_id = mk(0);
    let mut c = new_test_contract(&vault_id);
    let owner = c.own_get_owner().unwrap();
    let receiver = mk(9);
    let stale_owner = mk(10);
    let stale_receiver = mk(11);
    setup_env(&vault_id, &owner, vec![]);

    let owner_addr = account_id_to_address(&owner);
    let receiver_addr = account_id_to_address(&receiver);
    let stale_owner_addr = account_id_to_address(&stale_owner);
    let stale_receiver_addr = account_id_to_address(&stale_receiver);

    c.address_book.insert(stale_owner_addr, stale_owner);
    c.address_book.insert(stale_receiver_addr, stale_receiver);
    c.address_book.insert(owner_addr, owner.clone());
    c.address_book.insert(receiver_addr, receiver.clone());

    c.set_op_state(OpState::Withdrawing(WithdrawingState {
        op_id: 42,
        request_id: 42,
        index: 0,
        remaining: 5,
        collected: 0,
        receiver: receiver_addr,
        owner: owner_addr,
        escrow_shares: 7,
    }));

    assert_eq!(c.resolve_account(&owner_addr), owner);
    assert_eq!(c.resolve_account(&receiver_addr), receiver);
    assert!(!c.address_book.contains_key(&stale_owner_addr));
    assert!(!c.address_book.contains_key(&stale_receiver_addr));
}

/// Same AccountId always produces the same kernel Address (deterministic).
#[test]
fn address_mapping_is_deterministic() {
    use crate::convert::account_id_to_address;
    let vid = vault_id();
    setup_env(&vid, &vid, vec![]);
    let alice: AccountId = "alice.near".parse().unwrap();
    let addr1 = account_id_to_address(&alice);
    let addr2 = account_id_to_address(&alice);
    assert_eq!(addr1, addr2, "Same AccountId must map to same Address");
}

/// Different AccountIds produce different kernel Addresses.
#[test]
fn address_mapping_distinct_accounts_no_collision() {
    use crate::convert::account_id_to_address;
    use std::collections::HashSet;
    let vid = vault_id();
    setup_env(&vid, &vid, vec![]);

    let accounts: Vec<AccountId> = vec![
        "alice.near".parse().unwrap(),
        "bob.near".parse().unwrap(),
        "carol.near".parse().unwrap(),
        "alice.testnet".parse().unwrap(),
        "a.near".parse().unwrap(),
        "aa.near".parse().unwrap(),
        "aaa.near".parse().unwrap(),
        "alice-near.testnet".parse().unwrap(),
    ];

    let addresses: Vec<_> = accounts.iter().map(account_id_to_address).collect();
    let unique: HashSet<_> = addresses.iter().collect();
    assert_eq!(
        unique.len(),
        accounts.len(),
        "All distinct AccountIds must produce distinct Addresses"
    );
}

/// Similar AccountIds (prefix/suffix overlap) produce different addresses.
#[test]
fn address_mapping_similar_names_no_collision() {
    use crate::convert::account_id_to_address;
    let vid = vault_id();
    setup_env(&vid, &vid, vec![]);

    let a1: AccountId = "alice.near".parse().unwrap();
    let a2: AccountId = "alice.nea".parse().unwrap();
    let a3: AccountId = "lice.near".parse().unwrap();
    assert_ne!(account_id_to_address(&a1), account_id_to_address(&a2));
    assert_ne!(account_id_to_address(&a1), account_id_to_address(&a3));
    assert_ne!(account_id_to_address(&a2), account_id_to_address(&a3));
}

/// Domain separation: NEAR's mapping differs from a raw SHA256 (no domain prefix).
#[test]
fn address_mapping_is_domain_separated() {
    use crate::convert::account_id_to_address;
    let vid = vault_id();
    setup_env(&vid, &vid, vec![]);

    let alice: AccountId = "alice.near".parse().unwrap();
    let derived = account_id_to_address(&alice);

    // Raw SHA256 without domain prefix should differ
    let raw_hash = env::sha256(alice.as_bytes());
    let raw_addr = templar_vault_kernel::Address(raw_hash.as_slice().try_into().unwrap());
    assert_ne!(
        derived, raw_addr,
        "Domain-prefixed hash must differ from raw hash"
    );
}

/// Domain separation: NEAR domain prefix differs from Soroban's.
/// Even if the same string were hashed, different domain prefixes
/// guarantee different kernel Addresses across chains.
#[test]
fn address_mapping_cross_chain_domain_separation() {
    let vid = vault_id();
    setup_env(&vid, &vid, vec![]);

    let input = b"alice.near";
    let near_domain = b"templar:near:account-id";
    let soroban_domain = b"templar:soroban:address";

    let mut near_bytes = Vec::with_capacity(near_domain.len() + input.len());
    near_bytes.extend_from_slice(near_domain);
    near_bytes.extend_from_slice(input);
    let near_hash: [u8; 32] = env::sha256(&near_bytes).as_slice().try_into().unwrap();

    let mut soroban_bytes = Vec::with_capacity(soroban_domain.len() + input.len());
    soroban_bytes.extend_from_slice(soroban_domain);
    soroban_bytes.extend_from_slice(input);
    let soroban_hash: [u8; 32] = env::sha256(&soroban_bytes).as_slice().try_into().unwrap();

    assert_ne!(
        near_hash, soroban_hash,
        "Same input with different domain prefixes must produce different addresses"
    );
}

/// Escrow address (all zeros) never collides with any real account mapping.
#[test]
fn address_mapping_never_produces_escrow_address() {
    use crate::convert::account_id_to_address;
    let vid = vault_id();
    setup_env(&vid, &vid, vec![]);

    let escrow = templar_vault_kernel::Address([0u8; 32]);
    let accounts: Vec<AccountId> = vec![
        "alice.near".parse().unwrap(),
        "bob.near".parse().unwrap(),
        "vault.near".parse().unwrap(),
        "a.testnet".parse().unwrap(),
    ];
    for account in &accounts {
        assert_ne!(
            account_id_to_address(account),
            escrow,
            "account_id_to_address must never produce the all-zero escrow address for {:?}",
            account
        );
    }
}

#[test]
fn policy_supply_queue_roundtrips_vec_layout() {
    let source = vec![MarketId(1), MarketId(2)];
    let queue = crate::policy::SupplyQueue::from(source.clone());
    let roundtrip: Vec<MarketId> = queue.into();
    assert_eq!(roundtrip, source);
}

#[test]
fn policy_withdraw_route_roundtrips_vec_layout() {
    let source = vec![MarketId(3), MarketId(4)];
    let route = crate::policy::WithdrawRoute::from(source.clone());
    let roundtrip: Vec<MarketId> = route.into();
    assert_eq!(roundtrip, source);
}
