#![no_std]

extern crate alloc;
#[cfg(any(test, feature = "std", feature = "schemars", feature = "borsh-schema"))]
extern crate std;

pub mod abort;
pub mod actions;
pub mod address_book;
pub mod effects;
pub mod error;
pub mod fee;
pub mod math;
pub mod restrictions;
pub mod state;

#[doc(hidden)]
pub mod test_utils;
pub mod transitions;
pub mod types;
pub mod utils;

pub use actions::{
    apply_action, convert_to_assets, convert_to_assets_bounded, convert_to_assets_ceil,
    convert_to_assets_ceil_bounded, convert_to_shares, convert_to_shares_bounded,
    convert_to_shares_ceil, convert_to_shares_ceil_bounded, effective_totals, plan_idle_payout,
    preview_deposit_shares, preview_withdraw_assets, EffectiveTotals, IdlePayoutPlan, KernelAction,
    KernelResult, PayoutOutcome,
};
pub use address_book::AddressBook;
pub use fee::{Fee, FeeSlot, Fees, FeesSpec};
pub use math::number::Number;
pub use math::wad::{
    compute_fee_shares, compute_fee_shares_from_assets, compute_management_fee_shares,
    mul_div_ceil, mul_div_floor, mul_wad_floor, total_assets_for_fee_accrual, Wad, MAX_FEE_WAD,
    MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD, YEAR_NS,
};
pub use restrictions::{RestrictionKind, RestrictionMode, Restrictions};
pub use state::escrow::{
    apply_settlement, can_apply_settlement, compute_escrow_stats, find_by_owner, is_stale,
    settle_proportional, settle_proportional_raw, total_burn, total_refund, EscrowEntry,
    EscrowSettlement, EscrowStats, SettlementResult,
};
pub use state::op_state::{
    AllocatingState, AllocationPlanEntry, IdleState, OpState, PayoutState, RefreshingState,
    TargetId, WithdrawingState,
};
pub use state::queue::{
    can_enqueue, can_partially_satisfy, can_satisfy_withdrawal, compute_full_withdrawal,
    compute_idle_settlement, compute_partial_withdrawal, compute_queue_status, compute_settlement,
    compute_settlement_by_price, count_satisfiable, find_request_status, is_past_cooldown,
    is_valid_withdrawal_amount, PendingWithdrawal, QueueError, QueueStatus, WithdrawQueue,
    WithdrawalRequestStatus, WithdrawalResult, DEFAULT_COOLDOWN_NS, MAX_PENDING, MAX_QUEUE_LENGTH,
    MIN_WITHDRAWAL_ASSETS,
};
pub use state::vault::{FeeAccrualAnchor, VaultConfig, VaultState};
pub use transitions::{
    allocation_step_callback, complete_allocation, complete_refresh, payout_complete,
    refresh_step_callback, start_allocation, start_refresh, start_withdrawal, stop_withdrawal,
    withdrawal_collected, withdrawal_settled, withdrawal_step_callback, TransitionError,
    TransitionRes, TransitionResult, WithdrawalRequest,
};

#[cfg(kani)]
mod kani_proofs {
    use alloc::{vec, vec::Vec};

    use super::*;
    use crate::effects::{KernelEffect, KernelEvent};

    const MAX_AMOUNT: u128 = 32;
    const OWNER: Address = Address([0x11; 32]);
    const RECEIVER: Address = Address([0x22; 32]);
    const SELF: Address = Address([0x33; 32]);
    const SECOND_OWNER: Address = Address([0x44; 32]);
    const SECOND_RECEIVER: Address = Address([0x55; 32]);

    fn bounded_amount() -> u128 {
        let amount = kani::any::<u128>();
        kani::assume(amount <= MAX_AMOUNT);
        amount
    }

    fn nonzero_bounded_amount() -> u128 {
        let amount = bounded_amount();
        kani::assume(amount > 0);
        amount
    }

    fn zero_fee_config() -> VaultConfig {
        VaultConfig {
            fees: FeesSpec::zero(),
            min_withdrawal_assets: 0,
            withdrawal_cooldown_ns: 0,
            max_pending_withdrawals: 3,
            paused: false,
            virtual_shares: 0,
            virtual_assets: 0,
        }
    }

    #[cfg(all(
        feature = "action-refresh-fees",
        feature = "kani-expensive-vault-proofs"
    ))]
    fn bounded_fee_config() -> VaultConfig {
        VaultConfig {
            fees: FeesSpec::new(
                FeeSlot::new(Wad(Number::from(1u128)), Address([0x66; 32])),
                FeeSlot::new(Wad(Number::from(1u128)), Address([0x77; 32])),
                None,
            ),
            min_withdrawal_assets: 0,
            withdrawal_cooldown_ns: 0,
            max_pending_withdrawals: 3,
            paused: false,
            virtual_shares: 0,
            virtual_assets: 0,
        }
    }

    fn assert_accounting_invariant(state: &VaultState) {
        assert!(state.check_invariant());
        assert_eq!(
            state.total_assets,
            state.idle_assets + state.external_assets
        );
    }

    fn assert_asset_sum(state: &VaultState) {
        assert_eq!(
            state.total_assets,
            state.idle_assets + state.external_assets
        );
    }

    fn assert_address_eq(left: Address, right: Address) {
        let mut index = 0usize;
        while index < 32 {
            assert_eq!(left.0[index], right.0[index]);
            index += 1;
        }
    }

    fn address_eq(left: Address, right: Address) -> bool {
        let mut index = 0usize;
        while index < 32 {
            if left.0[index] != right.0[index] {
                return false;
            }
            index += 1;
        }
        true
    }

    fn bounded_state() -> VaultState {
        let idle = bounded_amount();
        let external = bounded_amount();
        let shares = bounded_amount();
        VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO)
    }

    fn allocation_plan(first: u128, second: u128) -> Vec<AllocationPlanEntry> {
        vec![
            AllocationPlanEntry::new(0, first),
            AllocationPlanEntry::new(1, second),
        ]
    }

    fn enqueue_withdrawal(
        state: &mut VaultState,
        owner: Address,
        receiver: Address,
        shares: u128,
        expected_assets: u128,
        requested_at_ns: TimestampNs,
    ) -> u64 {
        state
            .withdraw_queue
            .enqueue(owner, receiver, shares, expected_assets, requested_at_ns, 3)
            .unwrap()
    }

    fn refund_effect_sum(effects: &[KernelEffect], owner: Address) -> u128 {
        let mut total = 0u128;
        for effect in effects {
            if let KernelEffect::TransferShares { from, to, shares } = effect {
                if address_eq(*from, SELF) && address_eq(*to, owner) {
                    total += *shares;
                }
            }
        }
        total
    }

    fn burn_effect_sum(effects: &[KernelEffect]) -> u128 {
        let mut total = 0u128;
        for effect in effects {
            if let KernelEffect::BurnShares { owner, shares } = effect {
                if address_eq(*owner, SELF) {
                    total += *shares;
                }
            }
        }
        total
    }

    #[cfg(feature = "action-refresh-fees")]
    fn minted_effect_sum(effects: &[KernelEffect]) -> u128 {
        let mut total = 0u128;
        for effect in effects {
            if let KernelEffect::MintShares { shares, .. } = effect {
                total += *shares;
            }
        }
        total
    }

    fn payout_event(effects: &[KernelEffect]) -> Option<(bool, u128, u128, u128)> {
        for effect in effects {
            if let KernelEffect::EmitEvent {
                event:
                    KernelEvent::PayoutCompleted {
                        success,
                        burn_shares,
                        refund_shares,
                        amount,
                        ..
                    },
            } = effect
            {
                return Some((*success, *burn_shares, *refund_shares, *amount));
            }
        }
        None
    }

    #[kani::proof]
    fn bounded_initial_state_preserves_total_asset_invariant() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let shares = bounded_amount();

        let state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);

        assert!(state.check_invariant());
        assert_eq!(
            state.total_assets,
            state.idle_assets + state.external_assets
        );
        assert_eq!(state.total_shares, shares);
        assert_eq!(state.withdraw_queue.status().length, 0);
    }

    #[kani::proof]
    fn restore_to_idle_preserves_total_asset_invariant() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let restored = bounded_amount();
        let shares = bounded_amount();

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        state.restore_to_idle(restored);

        assert!(state.check_invariant());
        assert_eq!(state.idle_assets, idle + restored);
        assert_eq!(state.external_assets, external);
        assert_eq!(state.total_assets, idle + external + restored);
    }

    #[kani::proof]
    fn withdrawal_queue_enqueue_preserves_cached_escrow_and_claimability() {
        let shares = nonzero_bounded_amount();
        let expected_assets = bounded_amount();
        let mut queue = WithdrawQueue::new();

        let id = queue
            .enqueue(
                OWNER,
                RECEIVER,
                shares,
                expected_assets,
                TimestampNs::ZERO,
                3,
            )
            .unwrap();

        let status = queue.status();
        assert_eq!(id, 0);
        assert!(queue.check_invariants_with_max(3));
        assert_eq!(status.length, 1);
        assert_eq!(status.total_escrow_shares, shares);
        assert_eq!(status.total_expected_assets, expected_assets);
        assert!(queue.contains(id));
        assert!(queue.head().is_some());
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn two_entry_withdrawal_queue_preserves_cached_escrow_and_claimability() {
        let first_shares = nonzero_bounded_amount();
        let second_shares = nonzero_bounded_amount();
        let first_expected_assets = bounded_amount();
        let second_expected_assets = bounded_amount();
        let mut queue = WithdrawQueue::new();

        let first_id = queue
            .enqueue(
                OWNER,
                RECEIVER,
                first_shares,
                first_expected_assets,
                TimestampNs::ZERO,
                3,
            )
            .unwrap();
        let second_id = queue
            .enqueue(
                RECEIVER,
                OWNER,
                second_shares,
                second_expected_assets,
                TimestampNs::ZERO,
                3,
            )
            .unwrap();

        let status = queue.status();
        let first = queue
            .get(first_id)
            .expect("first withdrawal should be queued");
        let second = queue
            .get(second_id)
            .expect("second withdrawal should be queued");

        assert_eq!(first_id, 0);
        assert_eq!(second_id, 1);
        assert!(queue.check_invariants_with_max(3));
        assert_eq!(status.length, 2);
        assert_eq!(status.total_escrow_shares, first_shares + second_shares);
        assert_eq!(
            status.total_expected_assets,
            first_expected_assets + second_expected_assets
        );
        assert!(queue.contains(first_id));
        assert!(queue.contains(second_id));
        assert_eq!(queue.head().map(|(id, _)| id), Some(first_id));
        assert_eq!(first.escrow_shares, first_shares);
        assert_eq!(first.expected_assets, first_expected_assets);
        assert_eq!(second.escrow_shares, second_shares);
        assert_eq!(second.expected_assets, second_expected_assets);
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn two_entry_withdrawal_queue_dequeues_fifo_and_preserves_cache() {
        let first_shares = nonzero_bounded_amount();
        let second_shares = nonzero_bounded_amount();
        let first_expected_assets = bounded_amount();
        let second_expected_assets = bounded_amount();
        let mut queue = WithdrawQueue::new();

        let first_id = queue
            .enqueue(
                OWNER,
                RECEIVER,
                first_shares,
                first_expected_assets,
                TimestampNs::ZERO,
                3,
            )
            .unwrap();
        let second_id = queue
            .enqueue(
                RECEIVER,
                OWNER,
                second_shares,
                second_expected_assets,
                TimestampNs::ZERO,
                3,
            )
            .unwrap();

        let (dequeued_id, dequeued) = queue.dequeue().expect("first withdrawal should dequeue");
        let status = queue.status();

        assert_eq!(dequeued_id, first_id);
        assert_eq!(dequeued.escrow_shares, first_shares);
        assert_eq!(dequeued.expected_assets, first_expected_assets);
        assert!(queue.check_invariants_with_max(3));
        assert_eq!(status.length, 1);
        assert_eq!(status.total_escrow_shares, second_shares);
        assert_eq!(status.total_expected_assets, second_expected_assets);
        assert!(!queue.contains(first_id));
        assert!(queue.contains(second_id));
        assert_eq!(queue.head().map(|(id, _)| id), Some(second_id));
    }

    #[cfg(feature = "action-sync-external")]
    #[kani::proof]
    fn rebalance_withdraw_conserves_total_assets_and_moves_external_to_idle() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let shares = bounded_amount();
        let amount = bounded_amount();
        kani::assume(amount <= external);

        let state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        let before_total_assets = state.total_assets;
        let before_total_shares = state.total_shares;

        let result = match apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::rebalance_withdraw(0, amount, TimestampNs::ZERO),
        ) {
            Ok(result) => result,
            Err(_) => panic!("bounded rebalance withdraw should succeed"),
        };

        assert_eq!(
            result.state.total_assets,
            result.state.idle_assets + result.state.external_assets
        );
        assert_eq!(result.state.total_assets, before_total_assets);
        assert_eq!(result.state.total_shares, before_total_shares);
        assert_eq!(result.state.idle_assets, idle + amount);
        assert_eq!(result.state.external_assets, external - amount);
    }

    #[cfg(feature = "action-sync-external")]
    #[kani::proof]
    fn sync_external_assets_preserves_total_as_idle_plus_external() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let synced_external = bounded_amount();
        let shares = bounded_amount();
        let op_id = 7;

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id,
            index: 0,
            remaining: 0,
            plan: Vec::new(),
        });

        let result = match apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id, TimestampNs::ZERO),
        ) {
            Ok(result) => result,
            Err(_) => panic!("bounded sync external assets should succeed"),
        };

        assert_eq!(
            result.state.total_assets,
            result.state.idle_assets + result.state.external_assets
        );
        assert_eq!(result.state.idle_assets, idle);
        assert_eq!(result.state.external_assets, synced_external);
        assert_eq!(result.state.total_assets, idle + synced_external);
        assert_eq!(result.state.total_shares, shares);
    }

    #[cfg(feature = "action-sync-external")]
    #[kani::proof]
    fn bounded_sync_then_rebalance_conserves_accounting_across_actions() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let shares = bounded_amount();
        let synced_external = bounded_amount();
        let rebalance_amount = bounded_amount();
        let op_id = 9;
        kani::assume(rebalance_amount <= synced_external);

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id,
            index: 0,
            remaining: 0,
            plan: Vec::new(),
        });

        let synced = match apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id, TimestampNs::ZERO),
        ) {
            Ok(result) => result.state,
            Err(_) => panic!("bounded sync external assets should succeed"),
        };

        let rebalanced = match apply_action(
            synced,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::rebalance_withdraw(op_id, rebalance_amount, TimestampNs::ZERO),
        ) {
            Ok(result) => result.state,
            Err(_) => panic!("bounded rebalance withdraw should succeed after sync"),
        };

        assert_eq!(
            rebalanced.total_assets,
            rebalanced.idle_assets + rebalanced.external_assets
        );
        assert_eq!(rebalanced.total_shares, shares);
        assert_eq!(rebalanced.idle_assets, idle + rebalance_amount);
        assert_eq!(
            rebalanced.external_assets,
            synced_external - rebalance_amount
        );
        assert_eq!(rebalanced.total_assets, idle + synced_external);
    }

    #[cfg(all(
        feature = "action-allocation-lifecycle",
        feature = "action-sync-external",
        feature = "action-recovery"
    ))]
    #[kani::proof]
    #[kani::unwind(8)]
    fn allocation_partial_sync_then_abort_restores_unallocated_assets() {
        let idle = nonzero_bounded_amount();
        let external = bounded_amount();
        let shares = bounded_amount();
        let first = nonzero_bounded_amount();
        let second = bounded_amount();
        kani::assume(first + second <= idle);

        let op_id = 11;
        let state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        let started = apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::begin_allocating(
                op_id,
                allocation_plan(first, second),
                TimestampNs::ZERO,
            ),
        )
        .unwrap()
        .state;

        let stepped = allocation_step_callback(started.op_state.clone(), true, first, op_id)
            .unwrap()
            .new_state;
        let mut after_step = started;
        after_step.op_state = stepped;

        let synced = apply_action(
            after_step,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(external + first, op_id, TimestampNs::ZERO),
        )
        .unwrap()
        .state;

        let result = apply_action(
            synced,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::abort_allocating(op_id),
        )
        .unwrap();

        assert!(result.state.op_state.is_idle());
        assert_asset_sum(&result.state);
        assert_eq!(result.state.idle_assets, idle - first);
        assert_eq!(result.state.external_assets, external + first);
        assert_eq!(result.state.total_assets, idle + external);
        assert_eq!(result.state.total_shares, shares);
        assert_eq!(result.state.withdraw_queue.status().length, 0);
    }

    #[cfg(all(
        feature = "action-allocation-lifecycle",
        feature = "action-sync-external"
    ))]
    #[kani::proof]
    #[kani::unwind(8)]
    fn allocation_full_sync_then_finish_conserves_assets() {
        let idle = nonzero_bounded_amount();
        let external = bounded_amount();
        let shares = bounded_amount();
        let first = nonzero_bounded_amount();
        let second = bounded_amount();
        kani::assume(first + second <= idle);

        let op_id = 12;
        let state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        let started = apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::begin_allocating(
                op_id,
                allocation_plan(first, second),
                TimestampNs::ZERO,
            ),
        )
        .unwrap()
        .state;

        let stepped_once = allocation_step_callback(started.op_state.clone(), true, first, op_id)
            .unwrap()
            .new_state;
        let stepped_twice = if second > 0 {
            allocation_step_callback(stepped_once, true, second, op_id)
                .unwrap()
                .new_state
        } else {
            stepped_once
        };
        let mut after_steps = started;
        after_steps.op_state = stepped_twice;

        let synced = apply_action(
            after_steps,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(external + first + second, op_id, TimestampNs::ZERO),
        )
        .unwrap()
        .state;

        let result = apply_action(
            synced,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::finish_allocating(op_id, TimestampNs::ZERO),
        )
        .unwrap();

        assert!(result.state.op_state.is_idle());
        assert_asset_sum(&result.state);
        assert_eq!(result.state.idle_assets, idle - first - second);
        assert_eq!(result.state.external_assets, external + first + second);
        assert_eq!(result.state.total_assets, idle + external);
        assert_eq!(result.state.total_shares, shares);
    }

    #[cfg(all(
        feature = "action-allocation-lifecycle",
        feature = "action-sync-external",
        feature = "action-recovery"
    ))]
    #[kani::proof]
    fn allocation_wrong_op_id_is_rejected_without_progress() {
        let mut state = bounded_state();
        let op_id = 13;
        let wrong_op_id = 14;
        state.op_state = OpState::Allocating(AllocatingState {
            op_id,
            index: 0,
            remaining: 1,
            plan: allocation_plan(1, 0),
        });
        let baseline = state.clone();

        assert!(allocation_step_callback(state.op_state.clone(), true, 1, wrong_op_id).is_err());
        assert!(apply_action(
            state.clone(),
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(1, wrong_op_id, TimestampNs::ZERO),
        )
        .is_err());
        assert!(apply_action(
            state.clone(),
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::finish_allocating(wrong_op_id, TimestampNs::ZERO),
        )
        .is_err());
        assert!(apply_action(
            state.clone(),
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::abort_allocating(wrong_op_id),
        )
        .is_err());
        assert!(state == baseline);
    }

    #[cfg(all(feature = "action-recovery", feature = "kani-expensive-vault-proofs"))]
    #[kani::proof]
    #[kani::unwind(40)]
    fn abort_withdrawing_requires_fifo_head_identity_and_refunds_escrow() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let first_shares = nonzero_bounded_amount();
        let second_shares = nonzero_bounded_amount();
        let first_expected = nonzero_bounded_amount();
        let second_expected = nonzero_bounded_amount();
        let collected = bounded_amount();

        let mut state = VaultState::with_initial(
            idle + external,
            total_shares,
            idle,
            external,
            TimestampNs::ZERO,
        );
        let first_id = enqueue_withdrawal(
            &mut state,
            OWNER,
            RECEIVER,
            first_shares,
            first_expected,
            TimestampNs::ZERO,
        );
        let second_id = enqueue_withdrawal(
            &mut state,
            SECOND_OWNER,
            SECOND_RECEIVER,
            second_shares,
            second_expected,
            TimestampNs::ZERO,
        );

        let op_id = 21;
        let mismatched = WithdrawingState {
            op_id,
            request_id: second_id,
            index: 0,
            remaining: second_expected,
            collected,
            receiver: SECOND_RECEIVER,
            owner: SECOND_OWNER,
            escrow_shares: second_shares,
        };
        let mut mismatch_state = state.clone();
        mismatch_state.op_state = OpState::Withdrawing(mismatched);
        assert!(apply_action(
            mismatch_state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::abort_withdrawing(op_id),
        )
        .is_err());

        let mut valid_state = state.clone();
        valid_state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id,
            request_id: first_id,
            index: 0,
            remaining: first_expected,
            collected,
            receiver: RECEIVER,
            owner: OWNER,
            escrow_shares: first_shares,
        });
        let result = apply_action(
            valid_state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::abort_withdrawing(op_id),
        )
        .unwrap();

        assert!(result.state.op_state.is_idle());
        assert_asset_sum(&result.state);
        assert_eq!(result.state.idle_assets, idle + collected);
        assert_eq!(result.state.external_assets, external);
        assert_eq!(result.state.total_shares, total_shares);
        assert_eq!(refund_effect_sum(&result.effects, OWNER), first_shares);
        assert_eq!(result.state.withdraw_queue.status().length, 1);
        assert_eq!(
            result.state.withdraw_queue.head().map(|(id, _)| id),
            Some(second_id)
        );
    }

    #[kani::proof]
    fn withdrawal_collection_preserves_collected_plus_remaining() {
        let amount = nonzero_bounded_amount();
        let first_collect = bounded_amount();
        let burn_shares = bounded_amount();
        let escrow_shares = nonzero_bounded_amount();
        kani::assume(first_collect <= amount);
        kani::assume(burn_shares <= escrow_shares);

        let op_id = 31;
        let request = WithdrawalRequest {
            op_id,
            request_id: 0,
            amount,
            receiver: RECEIVER,
            owner: OWNER,
            escrow_shares,
        };

        let started = start_withdrawal(OpState::Idle, request).unwrap().new_state;
        let stepped = withdrawal_step_callback(started, op_id, first_collect)
            .unwrap()
            .new_state;
        let withdrawing = stepped.as_withdrawing().unwrap();
        assert_eq!(withdrawing.collected + withdrawing.remaining, amount);

        if withdrawing.remaining > 0 {
            assert!(withdrawal_collected(stepped.clone(), op_id, burn_shares).is_err());
        }

        let completed = withdrawal_step_callback(stepped, op_id, amount - first_collect)
            .unwrap()
            .new_state;
        let payout = withdrawal_collected(completed, op_id, burn_shares)
            .unwrap()
            .new_state;
        let payout = payout.as_payout().unwrap();
        assert_eq!(payout.amount, amount);
        assert_eq!(payout.burn_shares, burn_shares);
        assert!(payout.burn_shares <= payout.escrow_shares);
    }

    #[cfg(feature = "kani-expensive-vault-proofs")]
    fn payout_state(
        idle: u128,
        external: u128,
        total_shares: u128,
        escrow_shares: u128,
        burn_shares: u128,
        amount: u128,
        op_id: u64,
    ) -> VaultState {
        let mut state = VaultState::with_initial(
            idle + external,
            total_shares,
            idle,
            external,
            TimestampNs::ZERO,
        );
        let request_id = enqueue_withdrawal(
            &mut state,
            OWNER,
            RECEIVER,
            escrow_shares,
            amount,
            TimestampNs::ZERO,
        );
        state.op_state = OpState::Payout(PayoutState {
            op_id,
            request_id,
            receiver: RECEIVER,
            amount,
            owner: OWNER,
            escrow_shares,
            burn_shares,
        });
        state
    }

    #[cfg(feature = "kani-expensive-vault-proofs")]
    #[kani::proof]
    #[kani::unwind(40)]
    fn payout_success_settlement_conserves_assets_and_escrow() {
        let idle = nonzero_bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let escrow_shares = nonzero_bounded_amount();
        let burn_shares = bounded_amount();
        let amount = bounded_amount();
        kani::assume(burn_shares <= escrow_shares);
        kani::assume(burn_shares <= total_shares);
        kani::assume(amount <= idle);

        let op_id = 41;
        let success = apply_action(
            payout_state(
                idle,
                external,
                total_shares,
                escrow_shares,
                burn_shares,
                amount,
                op_id,
            ),
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::settle_payout(op_id, PayoutOutcome::Success),
        )
        .unwrap();
        assert!(success.state.op_state.is_idle());
        assert_asset_sum(&success.state);
        assert_eq!(success.state.idle_assets, idle - amount);
        assert_eq!(success.state.external_assets, external);
        assert_eq!(success.state.total_assets, idle + external - amount);
        assert_eq!(success.state.total_shares, total_shares - burn_shares);
        assert_eq!(success.state.withdraw_queue.status().length, 0);
        assert_eq!(burn_effect_sum(&success.effects), burn_shares);
        assert_eq!(
            refund_effect_sum(&success.effects, OWNER),
            escrow_shares - burn_shares
        );
        assert_eq!(
            payout_event(&success.effects),
            Some((true, burn_shares, escrow_shares - burn_shares, amount))
        );
    }

    #[cfg(feature = "kani-expensive-vault-proofs")]
    #[kani::proof]
    #[kani::unwind(40)]
    fn payout_failure_settlement_refunds_without_mutating_assets_or_shares() {
        let idle = nonzero_bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let escrow_shares = nonzero_bounded_amount();
        let burn_shares = bounded_amount();
        let amount = bounded_amount();
        kani::assume(burn_shares <= escrow_shares);
        kani::assume(burn_shares <= total_shares);
        kani::assume(amount <= idle);

        let op_id = 42;
        let failure = apply_action(
            payout_state(
                idle,
                external,
                total_shares,
                escrow_shares,
                burn_shares,
                amount,
                op_id,
            ),
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::settle_payout(op_id, PayoutOutcome::Failure),
        )
        .unwrap();
        assert!(failure.state.op_state.is_idle());
        assert_asset_sum(&failure.state);
        assert_eq!(failure.state.idle_assets, idle);
        assert_eq!(failure.state.external_assets, external);
        assert_eq!(failure.state.total_assets, idle + external);
        assert_eq!(failure.state.total_shares, total_shares);
        assert_eq!(failure.state.withdraw_queue.status().length, 0);
        assert_eq!(burn_effect_sum(&failure.effects), 0);
        assert_eq!(refund_effect_sum(&failure.effects, OWNER), escrow_shares);
        assert_eq!(
            payout_event(&failure.effects),
            Some((false, 0, escrow_shares, 0))
        );
    }

    #[cfg(all(feature = "action-recovery", feature = "kani-expensive-vault-proofs"))]
    #[kani::proof]
    #[kani::unwind(40)]
    fn emergency_reset_recovers_non_idle_states_without_share_or_queue_leakage() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let shares = nonzero_bounded_amount();
        let amount = bounded_amount();
        let escrow = nonzero_bounded_amount();
        let kind = kani::any::<u8>();
        kani::assume(kind < 4);

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        let op_id = 51;
        let expected_idle_restore;
        let expected_refund;
        let expected_queue_len;

        if kind == 0 {
            state.op_state = OpState::Allocating(AllocatingState {
                op_id,
                index: 0,
                remaining: amount,
                plan: allocation_plan(amount, 0),
            });
            expected_idle_restore = amount;
            expected_refund = 0;
            expected_queue_len = 0;
        } else if kind == 1 {
            let request_id = enqueue_withdrawal(
                &mut state,
                OWNER,
                RECEIVER,
                escrow,
                amount,
                TimestampNs::ZERO,
            );
            state.op_state = OpState::Withdrawing(WithdrawingState {
                op_id,
                request_id,
                index: 0,
                remaining: amount,
                collected: amount,
                receiver: RECEIVER,
                owner: OWNER,
                escrow_shares: escrow,
            });
            expected_idle_restore = amount;
            expected_refund = escrow;
            expected_queue_len = 0;
        } else if kind == 2 {
            let request_id = enqueue_withdrawal(
                &mut state,
                OWNER,
                RECEIVER,
                escrow,
                amount,
                TimestampNs::ZERO,
            );
            state.op_state = OpState::Payout(PayoutState {
                op_id,
                request_id,
                receiver: RECEIVER,
                amount,
                owner: OWNER,
                escrow_shares: escrow,
                burn_shares: 0,
            });
            expected_idle_restore = amount;
            expected_refund = escrow;
            expected_queue_len = 0;
        } else {
            state.op_state = OpState::Refreshing(RefreshingState {
                op_id,
                index: 0,
                plan: vec![0],
            });
            expected_idle_restore = 0;
            expected_refund = 0;
            expected_queue_len = 0;
        }

        let result = apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::emergency_reset(),
        )
        .unwrap();

        assert!(result.state.op_state.is_idle());
        assert_asset_sum(&result.state);
        assert_eq!(result.state.idle_assets, idle + expected_idle_restore);
        assert_eq!(result.state.external_assets, external);
        assert_eq!(result.state.total_shares, shares);
        assert_eq!(
            result.state.withdraw_queue.status().length,
            expected_queue_len
        );
        assert_eq!(refund_effect_sum(&result.effects, OWNER), expected_refund);
        assert_eq!(
            result.state.fee_anchor.total_assets,
            result.state.total_assets
        );
    }

    #[cfg(feature = "action-sync-external")]
    #[kani::proof]
    #[kani::unwind(40)]
    fn sync_external_assets_only_mutates_external_and_total_assets_for_allowed_ops() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let synced_external = bounded_amount();
        let shares = bounded_amount();
        let op_kind = kani::any::<u8>();
        kani::assume(op_kind < 3);

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        let op_id = 61;
        if op_kind == 0 {
            state.op_state = OpState::Allocating(AllocatingState {
                op_id,
                index: 1,
                remaining: 2,
                plan: allocation_plan(1, 1),
            });
        } else if op_kind == 1 {
            let request_id =
                enqueue_withdrawal(&mut state, OWNER, RECEIVER, 3, 4, TimestampNs::ZERO);
            state.op_state = OpState::Withdrawing(WithdrawingState {
                op_id,
                request_id,
                index: 1,
                remaining: 2,
                collected: 2,
                receiver: RECEIVER,
                owner: OWNER,
                escrow_shares: 3,
            });
        } else {
            state.op_state = OpState::Refreshing(RefreshingState {
                op_id,
                index: 1,
                plan: vec![7, 8],
            });
        }

        let before_idle = state.idle_assets;
        let before_shares = state.total_shares;
        let before_queue = state.withdraw_queue.status();
        let before_next_op_id = state.next_op_id;
        let before_fee_anchor_total_assets = state.fee_anchor.total_assets;
        let before_fee_anchor_timestamp = state.fee_anchor.timestamp_ns;
        let result = apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id, TimestampNs::ZERO),
        )
        .unwrap();

        assert_eq!(result.state.idle_assets, before_idle);
        assert_eq!(result.state.external_assets, synced_external);
        assert_eq!(result.state.total_assets, before_idle + synced_external);
        assert_eq!(result.state.total_shares, before_shares);
        assert_eq!(
            result.state.withdraw_queue.status().length,
            before_queue.length
        );
        assert_eq!(
            result.state.withdraw_queue.status().total_escrow_shares,
            before_queue.total_escrow_shares
        );
        assert_eq!(
            result.state.withdraw_queue.status().total_expected_assets,
            before_queue.total_expected_assets
        );
        assert_eq!(result.state.next_op_id, before_next_op_id);
        assert_eq!(
            result.state.fee_anchor.total_assets,
            before_fee_anchor_total_assets
        );
        assert!(result.state.fee_anchor.timestamp_ns == before_fee_anchor_timestamp);
        match &result.state.op_state {
            OpState::Allocating(alloc) => {
                assert_eq!(op_kind, 0);
                assert_eq!(alloc.op_id, op_id);
                assert_eq!(alloc.index, 1);
                assert_eq!(alloc.remaining, 2);
            }
            OpState::Withdrawing(withdraw) => {
                assert_eq!(op_kind, 1);
                assert_eq!(withdraw.op_id, op_id);
                assert_eq!(withdraw.index, 1);
                assert_eq!(withdraw.remaining, 2);
                assert_eq!(withdraw.collected, 2);
                assert_address_eq(withdraw.owner, OWNER);
                assert_address_eq(withdraw.receiver, RECEIVER);
                assert_eq!(withdraw.escrow_shares, 3);
            }
            OpState::Refreshing(refresh) => {
                assert_eq!(op_kind, 2);
                assert_eq!(refresh.op_id, op_id);
                assert_eq!(refresh.index, 1);
            }
            _ => panic!("sync must preserve active operation kind"),
        }
        assert_asset_sum(&result.state);

        let idle_state = VaultState::with_initial(
            before_idle + synced_external,
            before_shares,
            before_idle,
            synced_external,
            TimestampNs::ZERO,
        );
        assert!(apply_action(
            idle_state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id, TimestampNs::ZERO),
        )
        .is_err());
    }

    #[cfg(all(feature = "action-refresh-lifecycle", feature = "action-sync-external"))]
    #[kani::proof]
    #[kani::unwind(8)]
    fn refresh_lifecycle_mutates_only_external_assets_and_returns_idle() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let synced_external = bounded_amount();
        let shares = bounded_amount();
        let op_id = 71;

        let state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        let started = apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::begin_refreshing(op_id, vec![1, 2], TimestampNs::ZERO),
        )
        .unwrap()
        .state;
        assert!(started.op_state.is_refreshing());
        assert_eq!(started.idle_assets, idle);
        assert_eq!(started.external_assets, external);
        assert_eq!(started.total_assets, idle + external);
        assert_eq!(started.total_shares, shares);

        let synced = apply_action(
            started,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id, TimestampNs::ZERO),
        )
        .unwrap()
        .state;

        let result = apply_action(
            synced,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::finish_refreshing(op_id, TimestampNs::ZERO),
        )
        .unwrap();

        assert!(result.state.op_state.is_idle());
        assert_eq!(result.state.idle_assets, idle);
        assert_eq!(result.state.external_assets, synced_external);
        assert_eq!(result.state.total_assets, idle + synced_external);
        assert_eq!(result.state.total_shares, shares);
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-refresh-fees")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn refresh_fees_zero_fee_rates_only_update_anchor() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let shares = nonzero_bounded_amount();
        let anchor_assets = bounded_amount();
        let now = TimestampNs::from_nanos(1);

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        state.fee_anchor = FeeAccrualAnchor::new(anchor_assets, TimestampNs::ZERO);
        let before = state.clone();
        let before_queue = before.withdraw_queue.status();

        let result = apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::refresh_fees(now),
        )
        .unwrap();

        let minted = minted_effect_sum(&result.effects);
        assert_eq!(result.state.idle_assets, before.idle_assets);
        assert_eq!(result.state.external_assets, before.external_assets);
        assert_eq!(result.state.total_assets, before.total_assets);
        assert_eq!(minted, 0);
        assert_eq!(result.state.total_shares, before.total_shares);
        assert_eq!(
            result.state.fee_anchor.total_assets,
            result.state.total_assets
        );
        assert!(result.state.fee_anchor.timestamp_ns == now);
        assert!(result.state.fee_anchor.timestamp_ns > before.fee_anchor.timestamp_ns);
        assert!(result.state.op_state.is_idle());
        assert_eq!(
            result.state.withdraw_queue.status().length,
            before_queue.length
        );
        assert_eq!(
            result.state.withdraw_queue.status().total_escrow_shares,
            before_queue.total_escrow_shares
        );
        assert_eq!(
            result.state.withdraw_queue.status().total_expected_assets,
            before_queue.total_expected_assets
        );
        assert_eq!(result.state.next_op_id, before.next_op_id);
        assert_asset_sum(&result.state);
    }
}
pub use types::{ActualIdx, Address, AssetId, DurationNs, ExpectedIdx, KernelVersion, TimestampNs};
pub use utils::TimeGate;
