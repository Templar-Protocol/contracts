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
    #[cfg(feature = "action-sync-external")]
    use alloc::vec::Vec;

    use super::*;

    const MAX_AMOUNT: u128 = 32;
    const OWNER: Address = Address([0x11; 32]);
    const RECEIVER: Address = Address([0x22; 32]);
    #[cfg(feature = "action-sync-external")]
    const SELF: Address = Address([0x33; 32]);

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

    #[cfg(feature = "action-sync-external")]
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
}
pub use types::{ActualIdx, Address, AssetId, DurationNs, ExpectedIdx, KernelVersion, TimestampNs};
pub use utils::TimeGate;
