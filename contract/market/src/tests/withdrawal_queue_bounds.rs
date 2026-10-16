//! Bounds on `execute_next_supply_withdrawal_request`.
//!
//! A supply position whose `outgoing` was never settled by
//! `execute_next_supply_withdrawal_request_01_finalize` keeps a non-zero
//! `total_deposit()` — enough to re-enter the queue — while
//! `record_withdrawal_initial` scores it `EmptyPosition`. Two properties keep
//! such entries from stalling the queue for everyone else: the walk must charge
//! every visited entry against `batch_limit`, and the continuation must be
//! given gas proportional to the batch it has to settle.

use near_sdk::{
    mock::MockAction,
    test_utils::{get_created_receipts, VMContextBuilder},
    testing_env, AccountId, Gas, NearToken, VMContext,
};
use rstest::rstest;
use templar_common::{
    market::{MarketExternalInterface, YieldWeights},
    supply::Deposit,
};

use crate::Contract;

const FINALIZE_METHOD: &[u8] = b"execute_next_supply_withdrawal_request_01_finalize";
const SUPPLY: u128 = 1_000;

struct Harness {
    context: VMContext,
    contract: Contract,
}

impl Harness {
    fn new() -> Self {
        let context = VMContextBuilder::new()
            .block_timestamp(1_000_000_000_000)
            .account_balance(NearToken::from_near(1_000))
            .prepaid_gas(Gas::from_tgas(300))
            .build();
        testing_env!(context.clone());

        let contract = Contract::new(test_utils::market_configuration(
            "oracle.near".parse().unwrap(),
            "borrow.near".parse().unwrap(),
            "collateral.near".parse().unwrap(),
            "revenue.near".parse().unwrap(),
            YieldWeights::new_with_supply_weight(1),
        ));

        Self { context, contract }
    }

    fn tick(&mut self) {
        self.context.block_timestamp += 1_000_000;
        testing_env!(self.context.clone());
    }

    fn as_account(&mut self, account: &AccountId) {
        self.context.predecessor_account_id = account.clone();
        testing_env!(self.context.clone());
    }

    fn fund(&mut self, account: &AccountId, amount: u128) {
        let proof = self.contract.market.snapshot();
        let mut position = self
            .contract
            .market
            .get_or_create_supply_position_guard(proof, account.clone());
        let yield_proof = position.accumulate_yield();
        position.record_deposit(
            yield_proof,
            amount.into(),
            near_sdk::env::block_timestamp_ms(),
        );
    }

    fn enqueue(&mut self, account: &AccountId, amount: u128) {
        self.contract
            .market
            .withdrawal_queue
            .insert_or_update(account, amount.into());
    }

    fn execute(&mut self, batch_limit: u32) {
        self.tick();
        let _ = self
            .contract
            .execute_next_supply_withdrawal_request(Some(batch_limit));
    }

    fn execute_measuring_gas(&mut self, batch_limit: u32) -> Gas {
        self.tick();
        let before = near_sdk::env::used_gas();
        let _ = self
            .contract
            .execute_next_supply_withdrawal_request(Some(batch_limit));
        near_sdk::env::used_gas().saturating_sub(before)
    }

    fn queue_len(&self) -> u32 {
        self.contract.market.withdrawal_queue.len()
    }

    fn deposit_of(&self, account: &AccountId) -> Deposit {
        self.contract
            .market
            .supply_position_ref(account.clone())
            .unwrap()
            .inner()
            .get_deposit()
            .clone()
    }

    /// Settles a withdrawal in the outer receipt and then drops the
    /// continuation, exactly as an out-of-gas `..._01_finalize` does: the
    /// transfer stands, `outgoing` does not.
    fn strand(&mut self, account: &AccountId) {
        self.fund(account, SUPPLY);
        self.enqueue(account, SUPPLY);
        self.execute(1);
    }

    fn strand_wall(&mut self, width: u32) {
        // Strand each position while it is alone in the queue: an unbounded
        // walk would otherwise consume the wall as it is being built.
        for i in 0..width {
            self.strand(&account(i));
        }
        for i in 0..width {
            self.enqueue(&account(i), SUPPLY);
        }
    }
}

fn account(i: u32) -> AccountId {
    format!("a{i}.near").parse().unwrap()
}

/// Gas on the continuation scheduled by the most recent call, if any.
fn continuation_gas() -> Option<Gas> {
    get_created_receipts().into_iter().find_map(|receipt| {
        receipt.actions.into_iter().find_map(|action| match action {
            MockAction::FunctionCallWeight {
                method_name,
                prepaid_gas,
                ..
            } if method_name == FINALIZE_METHOD => Some(prepaid_gas),
            _ => None,
        })
    })
}

/// Total gas the call commits to the receipts it schedules. A batch is only
/// executable if this stays under the 300 TGas a transaction can prepay.
fn committed_gas() -> Gas {
    get_created_receipts()
        .into_iter()
        .flat_map(|receipt| receipt.actions)
        .fold(Gas::from_gas(0), |total, action| match action {
            MockAction::FunctionCallWeight { prepaid_gas, .. } => total.saturating_add(prepaid_gas),
            _ => total,
        })
}

#[test]
fn an_unsettled_withdrawal_leaves_a_requeueable_empty_position() {
    let mut harness = Harness::new();
    let account = account(0);
    harness.strand(&account);

    let deposit = harness.deposit_of(&account);
    assert_eq!(u128::from(deposit.active), 0);
    assert!(deposit.incoming.is_empty());
    assert_eq!(
        u128::from(deposit.outgoing),
        SUPPLY,
        "the dropped continuation must strand `outgoing`",
    );

    // `total_deposit()` counts `outgoing`, so the position still clears the
    // eligibility gate and re-enters the queue.
    harness.as_account(&account);
    harness
        .contract
        .create_supply_withdrawal_request(SUPPLY.into());
    assert_eq!(harness.queue_len(), 1);
}

#[rstest]
#[case(1)]
#[case(5)]
#[case(10)]
fn empty_positions_are_charged_against_the_batch_limit(#[case] batch_limit: u32) {
    const WALL: u32 = 40;

    let mut harness = Harness::new();
    harness.strand_wall(WALL);
    assert_eq!(harness.queue_len(), WALL);

    harness.execute(batch_limit);

    assert_eq!(
        harness.queue_len(),
        WALL - batch_limit,
        "the walk must stop after visiting `batch_limit` entries",
    );
}

#[rstest]
#[case(1)]
#[case(7)]
fn entries_without_a_position_are_charged_against_the_batch_limit(#[case] batch_limit: u32) {
    const WALL: u32 = 40;

    let mut harness = Harness::new();
    for i in 0..WALL {
        harness.enqueue(&account(i), SUPPLY);
    }

    harness.execute(batch_limit);

    assert_eq!(
        harness.queue_len(),
        WALL - batch_limit,
        "an entry whose position is gone must still be charged against the limit",
    );
}

#[test]
fn continuation_gas_scales_with_the_batch() {
    let mut harness = Harness::new();

    let single = account(0);
    harness.fund(&single, SUPPLY);
    harness.enqueue(&single, SUPPLY);
    harness.execute(1);
    let one = continuation_gas().expect("a fulfilled request must schedule a continuation");

    for i in 1..=4 {
        let account = account(i);
        harness.fund(&account, SUPPLY);
        harness.enqueue(&account, SUPPLY);
    }
    harness.execute(4);
    let four = continuation_gas().expect("a fulfilled batch must schedule a continuation");

    assert!(
        four > one,
        "a four-entry batch must be given more gas than a one-entry batch ({one:?} vs {four:?})",
    );
}

#[test]
fn a_full_batch_stays_within_the_transaction_gas_limit() {
    const BATCH: u32 = 12;

    let mut harness = Harness::new();
    harness.tick();
    for i in 0..BATCH {
        let account = account(i);
        harness.fund(&account, SUPPLY);
        harness.enqueue(&account, SUPPLY);
    }
    harness.execute(BATCH);

    assert!(
        committed_gas() < Gas::from_tgas(300),
        "a {BATCH}-entry batch commits {:?}, which no transaction could prepay",
        committed_gas(),
    );
}

#[test]
fn a_wall_of_empty_positions_drains_over_repeated_calls() {
    const WALL: u32 = 40;
    const BATCH: u32 = 10;

    let mut harness = Harness::new();
    harness.strand_wall(WALL);

    let mut calls = 0;
    while harness.queue_len() > 0 {
        assert!(calls < 20, "the wall must drain in bounded calls");
        harness.execute(BATCH);
        calls += 1;
    }

    assert_eq!(calls, WALL / BATCH);
}

#[test]
fn a_real_request_behind_a_wall_is_eventually_fulfilled() {
    const WALL: u32 = 30;

    let mut harness = Harness::new();
    harness.strand_wall(WALL);

    let supplier = account(WALL);
    harness.fund(&supplier, SUPPLY);
    harness.enqueue(&supplier, SUPPLY);

    let mut calls = 0;
    while u128::from(harness.deposit_of(&supplier).outgoing) == 0 {
        assert!(
            calls < 20,
            "a wall of empty positions must not stall the queue"
        );
        harness.execute(10);
        calls += 1;
    }

    assert_eq!(u128::from(harness.deposit_of(&supplier).outgoing), SUPPLY);
    assert_eq!(harness.queue_len(), 0);
}

#[rstest]
#[case(1)]
#[case(3)]
#[case(5)]
fn real_requests_still_fill_the_batch(#[case] batch_limit: u32) {
    const SUPPLIERS: u32 = 10;

    let mut harness = Harness::new();
    for i in 0..SUPPLIERS {
        let account = account(i);
        harness.fund(&account, SUPPLY);
        harness.enqueue(&account, SUPPLY);
    }

    harness.execute(batch_limit);

    assert_eq!(harness.queue_len(), SUPPLIERS - batch_limit);
}

#[test]
fn a_zero_batch_limit_visits_nothing() {
    const WALL: u32 = 5;

    let mut harness = Harness::new();
    harness.strand_wall(WALL);

    harness.execute(0);

    assert_eq!(harness.queue_len(), WALL);
}

#[test]
fn the_cost_of_a_call_is_bounded_by_the_batch_not_the_queue() {
    const WALL: u32 = 40;
    const NARROW: u32 = 10;
    const BATCH: u32 = 5;

    let mut harness = Harness::new();
    for i in 0..WALL {
        harness.strand(&account(i));
    }

    // Both measurements run against the same snapshot history, so the only
    // variable is how many entries sit in front of the batch.
    for i in 0..NARROW {
        harness.enqueue(&account(i), SUPPLY);
    }
    let narrow = harness.execute_measuring_gas(BATCH);
    while harness.queue_len() > 0 {
        harness.execute(BATCH);
    }

    for i in 0..WALL {
        harness.enqueue(&account(i), SUPPLY);
    }
    let wide = harness.execute_measuring_gas(BATCH);

    assert!(
        wide < Gas::from_gas(narrow.as_gas().saturating_mul(2)),
        "a {WALL}-entry queue cost {wide:?} against {narrow:?} for {NARROW}:          the walk is priced by the queue, not the batch",
    );
}

#[rstest]
#[case(0)]
#[case(1)]
#[case(4)]
#[case(32)]
fn continuation_gas_is_linear_in_the_batch(#[case] resolutions: usize) {
    let base = Contract::gas_execute_next_supply_withdrawal_request_01_finalize(0);
    let step =
        Contract::gas_execute_next_supply_withdrawal_request_01_finalize(1).saturating_sub(base);

    assert_eq!(
        Contract::gas_execute_next_supply_withdrawal_request_01_finalize(resolutions),
        base.saturating_add(Gas::from_gas(
            step.as_gas().saturating_mul(resolutions as u64)
        )),
    );
}

#[test]
fn continuation_gas_is_strictly_increasing() {
    for resolutions in 0..16 {
        assert!(
            Contract::gas_execute_next_supply_withdrawal_request_01_finalize(resolutions + 1)
                > Contract::gas_execute_next_supply_withdrawal_request_01_finalize(resolutions),
        );
    }
}

#[test]
fn continuation_gas_saturates_rather_than_wrapping() {
    let huge = Contract::gas_execute_next_supply_withdrawal_request_01_finalize(usize::MAX);
    let small = Contract::gas_execute_next_supply_withdrawal_request_01_finalize(1);
    assert!(
        huge > small,
        "a pathological batch must not wrap to a tiny budget"
    );
}

/// The batch that used to fit in a flat 8 TGas now asks for more than that,
/// which is the whole point: the flat budget was short.
#[test]
fn a_modest_batch_outgrows_the_former_flat_budget() {
    assert!(
        Contract::gas_execute_next_supply_withdrawal_request_01_finalize(6) > Gas::from_tgas(8),
    );
}

/// The continuation used to get a flat 8 TGas against a measured ~5.8 TGas.
/// Making the budget scale must not quietly shrink the single-request case,
/// which is the default and by far the most travelled path.
#[test]
fn the_single_request_budget_does_not_regress() {
    assert!(
        Contract::gas_execute_next_supply_withdrawal_request_01_finalize(1) >= Gas::from_tgas(8),
    );
}
