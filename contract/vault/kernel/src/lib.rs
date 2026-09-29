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

/// Whether the `action-recovery` feature is enabled in this resolved kernel build.
pub const ACTION_RECOVERY_ENABLED: bool = cfg!(feature = "action-recovery");
/// Whether the `action-sync-external` feature is enabled in this resolved kernel build.
pub const ACTION_SYNC_EXTERNAL_ENABLED: bool = cfg!(feature = "action-sync-external");
/// Whether the `action-refresh-fees` feature is enabled in this resolved kernel build.
pub const ACTION_REFRESH_FEES_ENABLED: bool = cfg!(feature = "action-refresh-fees");
/// Whether the `action-allocation-lifecycle` feature is enabled in this resolved kernel build.
pub const ACTION_ALLOCATION_LIFECYCLE_ENABLED: bool = cfg!(feature = "action-allocation-lifecycle");
/// Whether the `action-refresh-lifecycle` feature is enabled in this resolved kernel build.
pub const ACTION_REFRESH_LIFECYCLE_ENABLED: bool = cfg!(feature = "action-refresh-lifecycle");
/// Whether the `action-pause` feature is enabled in this resolved kernel build.
pub const ACTION_PAUSE_ENABLED: bool = cfg!(feature = "action-pause");
/// Whether the `action-epoch-settlement` feature is enabled in this resolved kernel build.
pub const ACTION_EPOCH_SETTLEMENT_ENABLED: bool = cfg!(feature = "action-epoch-settlement");

// The Soroban and NEAR wire formats contain different effect variants and
// must never be unified into one artifact. Other codec/schema features are
// valid for host tooling and are verified in the explicit artifact-profile
// build matrix.
#[cfg(all(target_arch = "wasm32", feature = "soroban", feature = "near"))]
compile_error!("unsafe codec profile: soroban + near shifts KernelEffect discriminants and adds NEAR-only effects to a Soroban artifact");
#[cfg(all(
    target_arch = "wasm32",
    any(feature = "borsh", feature = "borsh-schema"),
    feature = "soroban"
))]
compile_error!("unsafe codec profile: borsh codecs cannot be linked into a Soroban artifact");
#[cfg(all(target_arch = "wasm32", feature = "borsh", feature = "std"))]
compile_error!("unsafe codec profile: borsh + std enables an unstable duplicate schema path");

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
#[cfg(feature = "action-epoch-settlement")]
pub use state::deposits::{PendingDeposit, MAX_PENDING_DEPOSITS};

#[cfg(feature = "action-epoch-settlement")]
pub use state::escrow::{
    apply_settlement, can_apply_settlement, compute_escrow_stats, find_by_owner, is_stale,
    total_burn, total_refund, EscrowEntry, EscrowSettlement, EscrowStats, SettlementResult,
};
#[cfg(not(feature = "action-epoch-settlement"))]
pub use state::escrow::{
    apply_settlement, can_apply_settlement, compute_escrow_stats, find_by_owner, is_stale,
    settle_proportional, settle_proportional_raw, total_burn, total_refund, EscrowEntry,
    EscrowSettlement, EscrowStats, SettlementResult,
};

pub use state::op_state::{
    AllocatingState, AllocationPlanEntry, IdleState, OpState, PayoutState, RefreshingState,
    TargetId, WithdrawingState,
};

#[cfg(feature = "action-epoch-settlement")]
pub use state::queue::{
    can_enqueue, can_partially_satisfy, can_satisfy_withdrawal, compute_full_withdrawal,
    compute_idle_settlement, compute_partial_withdrawal, compute_queue_status, compute_settlement,
    compute_settlement_by_price, count_satisfiable, find_request_status, is_past_cooldown,
    is_valid_withdrawal_amount, settled_claim, PendingWithdrawal, QueueError, QueueStatus,
    WithdrawQueue, WithdrawalRequestStatus, WithdrawalResult, DEFAULT_COOLDOWN_NS, MAX_PENDING,
    MAX_QUEUE_LENGTH, MIN_WITHDRAWAL_ASSETS,
};
#[cfg(not(feature = "action-epoch-settlement"))]
pub use state::queue::{
    can_enqueue, can_partially_satisfy, can_satisfy_withdrawal, compute_full_withdrawal,
    compute_idle_settlement, compute_partial_withdrawal, compute_queue_status, compute_settlement,
    compute_settlement_by_price, count_satisfiable, find_request_status, is_past_cooldown,
    is_valid_withdrawal_amount, PendingWithdrawal, QueueError, QueueStatus, WithdrawQueue,
    WithdrawalRequestStatus, WithdrawalResult, DEFAULT_COOLDOWN_NS, MAX_PENDING, MAX_QUEUE_LENGTH,
    MIN_WITHDRAWAL_ASSETS,
};

#[cfg(feature = "action-epoch-settlement")]
pub use state::settlement::{
    EpochId, EpochPhase, EpochSnapshot, EpochState, RequestError, SettlementRejection,
    ValuationReportRef, FIRST_SETTLEMENT_EPOCH, MIGRATION_INTAKE_EPOCH,
};

pub use state::vault::{FeeAccrualAnchor, VaultConfig, VaultState};
pub use transitions::{
    allocation_step_callback, complete_allocation, complete_refresh, payout_complete,
    refresh_step_callback, start_allocation, start_refresh, start_withdrawal, stop_withdrawal,
    withdrawal_collected, withdrawal_settled, withdrawal_step_callback, TransitionError,
    TransitionRes, TransitionResult, WithdrawalRequest,
};

#[cfg(all(kani, feature = "action-epoch-settlement"))]
mod epoch_kani_proofs {
    use alloc::{vec, vec::Vec};

    use super::*;
    #[cfg(feature = "action-recovery")]
    use crate::actions::plan_emergency_reset;
    use crate::state::queue::settled_claim;
    use crate::effects::KernelEffect;

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

    #[cfg(feature = "action-refresh-fees")]
    fn active_fee_config() -> VaultConfig {
        VaultConfig {
            fees: FeesSpec::new(
                FeeSlot::new(Wad::one() / 10, Address([0x66; 32])),
                FeeSlot::zero(),
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
        assert_eq!(left.0[0], right.0[0]);
        assert_eq!(left.0[1], right.0[1]);
        assert_eq!(left.0[2], right.0[2]);
        assert_eq!(left.0[3], right.0[3]);
        assert_eq!(left.0[4], right.0[4]);
        assert_eq!(left.0[5], right.0[5]);
        assert_eq!(left.0[6], right.0[6]);
        assert_eq!(left.0[7], right.0[7]);
        assert_eq!(left.0[8], right.0[8]);
        assert_eq!(left.0[9], right.0[9]);
        assert_eq!(left.0[10], right.0[10]);
        assert_eq!(left.0[11], right.0[11]);
        assert_eq!(left.0[12], right.0[12]);
        assert_eq!(left.0[13], right.0[13]);
        assert_eq!(left.0[14], right.0[14]);
        assert_eq!(left.0[15], right.0[15]);
        assert_eq!(left.0[16], right.0[16]);
        assert_eq!(left.0[17], right.0[17]);
        assert_eq!(left.0[18], right.0[18]);
        assert_eq!(left.0[19], right.0[19]);
        assert_eq!(left.0[20], right.0[20]);
        assert_eq!(left.0[21], right.0[21]);
        assert_eq!(left.0[22], right.0[22]);
        assert_eq!(left.0[23], right.0[23]);
        assert_eq!(left.0[24], right.0[24]);
        assert_eq!(left.0[25], right.0[25]);
        assert_eq!(left.0[26], right.0[26]);
        assert_eq!(left.0[27], right.0[27]);
        assert_eq!(left.0[28], right.0[28]);
        assert_eq!(left.0[29], right.0[29]);
        assert_eq!(left.0[30], right.0[30]);
        assert_eq!(left.0[31], right.0[31]);
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

    /// Maximum accepted valuation-report age used by settlement proofs.
    const MAX_SETTLEMENT_AGE_NS: u64 = 60_000_000_000;

    /// Queue an unpriced withdrawal bound to the epoch accepting intake. The
    /// request carries escrowed shares and a caller-declared slippage floor;
    /// there is no asset figure to quote at request time.
    fn enqueue_withdrawal(
        state: &mut VaultState,
        owner: Address,
        receiver: Address,
        shares: u128,
        min_assets_out: u128,
        requested_at_ns: TimestampNs,
    ) -> u64 {
        let epoch_id = state
            .epoch
            .intake_epoch_id()
            .expect("epoch intake must be open");
        state
            .withdraw_queue
            .enqueue(
                owner,
                receiver,
                shares,
                min_assets_out,
                requested_at_ns,
                epoch_id,
                3,
            )
            .unwrap()
    }

    /// Settle the vault's open epoch against a bounded accepted report so the
    /// bound snapshot becomes the only source of payout claims.
    fn settle_epoch(state: &mut VaultState, settlement_nav: u128, eligible_supply: u128) {
        let cutoff_ns = TimestampNs::from_nanos(1_000_000_000);
        let report = ValuationReportRef {
            report_seq: 1,
            as_of_ns: cutoff_ns,
            report_hash: [9u8; 32],
        };
        let cutoff_state = state.epoch.begin_cutoff(cutoff_ns).expect("cutoff accepted");
        let snapshot = cutoff_state
            .build_settlement_snapshot(
                &report,
                settlement_nav,
                eligible_supply,
                cutoff_ns,
                MAX_SETTLEMENT_AGE_NS,
            )
            .expect("accepted report settles the epoch");
        state.epoch = cutoff_state
            .apply_settled(&snapshot)
            .expect("settlement recorded");
        assert!(state.epoch.check_invariants());
    }

    fn assert_transfer_shares_effect(
        effect: &KernelEffect,
        expected_from: Address,
        expected_to: Address,
        expected_shares: u128,
    ) {
        match effect {
            KernelEffect::TransferShares { from, to, shares } => {
                assert_address_eq(*from, expected_from);
                assert_address_eq(*to, expected_to);
                assert_eq!(*shares, expected_shares);
            }
            _ => panic!("expected transfer shares effect"),
        }
    }

    fn assert_emit_event_effect(effect: &KernelEffect) {
        match effect {
            KernelEffect::EmitEvent { .. } => {}
            _ => panic!("expected emit event effect"),
        }
    }

    #[cfg(feature = "action-refresh-fees")]
    fn assert_mint_shares_effect(effect: &KernelEffect) -> u128 {
        match effect {
            KernelEffect::MintShares { shares, .. } => {
                assert!(*shares > 0);
                *shares
            }
            _ => panic!("refresh fees must not move assets or non-fee shares"),
        }
    }

    fn assert_refund_owner_is_owner(refund_owner: Option<Address>) {
        match refund_owner {
            Some(owner) => assert_address_eq(owner, OWNER),
            None => panic!("expected refund owner"),
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
    fn withdrawal_queue_enqueue_preserves_cached_escrow_and_leaves_request_unpriced() {
        let shares = nonzero_bounded_amount();
        let min_assets_out = bounded_amount();
        let available_assets = bounded_amount();
        let mut queue = WithdrawQueue::new();

        let id = queue
            .enqueue(
                OWNER,
                RECEIVER,
                shares,
                min_assets_out,
                TimestampNs::ZERO,
                EpochId::FIRST_SETTLEMENT,
                3,
            )
            .unwrap();

        let status = queue.status();
        assert_eq!(id, 0);
        assert!(queue.check_invariants_with_max(3));
        assert_eq!(status.length, 1);
        assert_eq!(status.total_escrow_shares, shares);
        assert!(queue.contains(id));

        let (_, head) = queue.head().expect("queued request must be head");
        assert_eq!(head.escrow_shares, shares);
        assert_eq!(head.min_assets_out, min_assets_out);
        assert_eq!(head.epoch_id, EpochId::FIRST_SETTLEMENT);
        // No accepted settlement exists, so escrow cannot be paid out.
        assert_eq!(settled_claim(head, &EpochState::genesis()), None);
        assert_eq!(
            compute_full_withdrawal(head, &EpochState::genesis(), available_assets),
            None
        );
        assert_eq!(
            compute_partial_withdrawal(head, &EpochState::genesis(), available_assets)
                .settlement
                .to_burn,
            0
        );
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn two_entry_withdrawal_queue_preserves_share_denominated_escrow_and_fifo() {
        let first_shares = nonzero_bounded_amount();
        let second_shares = nonzero_bounded_amount();
        let mut queue = WithdrawQueue::new();

        let first_id = queue
            .enqueue(
                OWNER,
                RECEIVER,
                first_shares,
                0,
                TimestampNs::ZERO,
                EpochId::FIRST_SETTLEMENT,
                3,
            )
            .unwrap();
        let second_id = queue
            .enqueue(
                RECEIVER,
                OWNER,
                second_shares,
                0,
                TimestampNs::ZERO,
                EpochId::FIRST_SETTLEMENT,
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
        assert!(queue.contains(first_id));
        assert!(queue.contains(second_id));
        assert_eq!(queue.head().map(|(id, _)| id), Some(first_id));
        assert_eq!(first.escrow_shares, first_shares);
        assert_eq!(second.escrow_shares, second_shares);
        // Neither entry carries an asset claim, and the queue exposes none.
        assert_eq!(settled_claim(first, &EpochState::genesis()), None);
        assert_eq!(settled_claim(second, &EpochState::genesis()), None);
        assert_eq!(
            compute_queue_status(queue.pending_withdrawals().values()),
            status
        );
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn two_entry_withdrawal_queue_dequeues_fifo_and_preserves_cache() {
        let first_shares = nonzero_bounded_amount();
        let second_shares = nonzero_bounded_amount();
        let mut queue = WithdrawQueue::new();

        let first_id = queue
            .enqueue(
                OWNER,
                RECEIVER,
                first_shares,
                0,
                TimestampNs::ZERO,
                EpochId::FIRST_SETTLEMENT,
                3,
            )
            .unwrap();
        let second_id = queue
            .enqueue(
                RECEIVER,
                OWNER,
                second_shares,
                0,
                TimestampNs::ZERO,
                EpochId::FIRST_SETTLEMENT,
                3,
            )
            .unwrap();

        let (dequeued_id, dequeued) = queue.dequeue().expect("first withdrawal should dequeue");
        let status = queue.status();

        assert_eq!(dequeued_id, first_id);
        assert_eq!(dequeued.escrow_shares, first_shares);
        assert!(queue.check_invariants_with_max(3));
        assert_eq!(status.length, 1);
        assert_eq!(status.total_escrow_shares, second_shares);
        assert!(!queue.contains(first_id));
        assert!(queue.contains(second_id));
        assert_eq!(queue.head().map(|(id, _)| id), Some(second_id));
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn unsettled_withdrawal_has_no_payout_claim_at_all() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let escrow_shares = nonzero_bounded_amount();
        let available_assets = bounded_amount();
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
            0,
            TimestampNs::ZERO,
        );
        let head = state
            .withdraw_queue
            .get(request_id)
            .expect("withdrawal must remain queued");
        assert_eq!(settled_claim(head, &state.epoch), None);
        assert_eq!(state.withdraw_queue.settled_head_claim(&state.epoch), None);
        assert_eq!(
            compute_full_withdrawal(head, &state.epoch, available_assets),
            None
        );
        assert!(state.withdraw_queue.check_invariants());
        assert_asset_sum(&state);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn settled_snapshot_claim_is_the_only_payout_figure() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let escrow_shares = nonzero_bounded_amount();
        let settlement_nav = bounded_amount();
        let eligible_supply = nonzero_bounded_amount();
        let available_assets = bounded_amount();
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
            0,
            TimestampNs::ZERO,
        );
        settle_epoch(&mut state, settlement_nav, eligible_supply);
        let head = state
            .withdraw_queue
            .get(request_id)
            .expect("settlement must not drop the queued request");
        let snapshot = state
            .epoch
            .last_settled
            .as_ref()
            .expect("settled epoch binds a snapshot");
        let derived = snapshot
            .claim_for(head.escrow_shares)
            .expect("bounded claim is representable");
        assert_eq!(settled_claim(head, &state.epoch), Some(derived));
        assert_eq!(
            state.withdraw_queue.settled_head_claim(&state.epoch),
            Some(derived)
        );
        match compute_full_withdrawal(head, &state.epoch, available_assets) {
            Some(result) => {
                assert_eq!(result.assets_out, derived);
                assert_eq!(result.settlement.to_burn, head.escrow_shares);
                assert_eq!(result.settlement.refund, 0);
            }
            None => assert!(available_assets < derived),
        }
        let partial = compute_partial_withdrawal(head, &state.epoch, available_assets);
        assert!(partial.assets_out <= derived);
        assert_eq!(
            partial.settlement.to_burn + partial.settlement.refund,
            head.escrow_shares
        );
        assert!(state.withdraw_queue.check_invariants());
        assert_asset_sum(&state);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn settled_snapshot_cannot_source_another_epoch_claim() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let escrow_shares = nonzero_bounded_amount();
        let settlement_nav = bounded_amount();
        let eligible_supply = nonzero_bounded_amount();
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
            0,
            TimestampNs::ZERO,
        );
        settle_epoch(&mut state, settlement_nav, eligible_supply);
        let snapshot = state
            .epoch
            .last_settled
            .as_ref()
            .expect("settled epoch binds a snapshot");
        assert!(snapshot.epoch_id().is_settlement_epoch());
        assert_eq!(
            state
                .epoch
                .settled_claim_for(snapshot.epoch_id().checked_next().unwrap(), escrow_shares),
            None,
            "the epoch after the bound snapshot cannot be priced by it"
        );
        assert_eq!(
            state.epoch.settled_claim_for(EpochId::MIGRATION_INTAKE, escrow_shares),
            snapshot.claim_for(escrow_shares),
            "only the first settlement epoch can price migrated intake"
        );
        let head = state
            .withdraw_queue
            .get(request_id)
            .expect("settlement must not drop the queued request");
        assert!(snapshot.epoch_id() > EpochId::FIRST_SETTLEMENT || head.epoch_id == snapshot.epoch_id());
        assert!(state.withdraw_queue.check_invariants());
        assert_asset_sum(&state);
    }

    #[kani::proof]
    fn escrow_settlement_must_consume_every_escrowed_share() {
        let escrow_shares = bounded_amount();
        let to_burn = bounded_amount();
        let refund = bounded_amount();
        let entry = EscrowEntry::new(OWNER, escrow_shares, TimestampNs::ZERO);
        let settlement = EscrowSettlement::partial(to_burn, refund);

        // Escrow is bounded well below overflow here, so the exact-partition
        // test is decidable: burn + refund must equal escrow exactly.
        let exact = to_burn + refund == escrow_shares;
        assert_eq!(can_apply_settlement(&entry, &settlement), exact);
        match apply_settlement(&entry, &settlement) {
            Some(result) => {
                assert!(exact);
                assert_eq!(result.burned, to_burn);
                assert_eq!(result.refunded, refund);
                assert_eq!(result.burned + result.refunded, escrow_shares);
            }
            None => assert!(!exact),
        }
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn unsettled_withdrawal_can_only_be_refunded_in_full() {
        let escrow_shares = nonzero_bounded_amount();
        let entry = EscrowEntry::new(OWNER, escrow_shares, TimestampNs::ZERO);

        // Cancellation of an unsettled request releases the full escrow and
        // burns nothing. This is the only lawful settlement.
        let refund_all = EscrowSettlement::refund_all(escrow_shares);
        assert!(can_apply_settlement(&entry, &refund_all));
        let cancelled = apply_settlement(&entry, &refund_all)
            .expect("cancellation must release all escrowed shares");
        assert_eq!(cancelled.burned, 0);
        assert_eq!(cancelled.refunded, escrow_shares);
        assert_eq!(cancelled.burned + cancelled.refunded, escrow_shares);

        // Any settlement that burns shares while stranding the remainder is
        // rejected, so an unsettled request can never be partially burned.
        let burned = bounded_amount();
        let refund = bounded_amount();
        let settlement = EscrowSettlement::partial(burned, refund);
        if burned > 0 && burned + refund != escrow_shares {
            assert!(!can_apply_settlement(&entry, &settlement));
            assert_eq!(apply_settlement(&entry, &settlement), None);
        }
        if can_apply_settlement(&entry, &settlement) {
            assert_eq!(burned + refund, escrow_shares);
            let priced = apply_settlement(&entry, &settlement)
                .expect("exact settlements are always priced");
            assert_eq!(priced.burned + priced.refunded, escrow_shares);
        }
        // The settlement helpers are pure arithmetic validators: they price
        // a settlement against an entry and never mutate it. Consumption of
        // a settled or cancelled request is enforced by the FIFO queue
        // lifecycle exercised by the queue/withdrawal unit, property, and
        // regression-equivalent suites: removing a queued request repairs
        // the escrow caches and makes every re-settlement or
        // re-cancellation of the same request a no-op.
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

    #[kani::proof]
    fn withdrawal_collection_preserves_collected_plus_remaining() {
        let amount = nonzero_bounded_amount();
        let first_collect = bounded_amount();
        kani::assume(first_collect <= amount);

        let op_id = 31;
        let request = WithdrawalRequest {
            op_id,
            request_id: 0,
            amount,
            receiver: RECEIVER,
            owner: OWNER,
            escrow_shares: nonzero_bounded_amount(),
        };

        let started = start_withdrawal(OpState::Idle, request)
            .unwrap()
            .new_state;
        let stepped = withdrawal_step_callback(started, op_id, first_collect)
            .unwrap()
            .new_state;
        let completed =
            withdrawal_step_callback(stepped.clone(), op_id, amount - first_collect)
                .unwrap()
                .new_state;
        let withdrawing = completed.as_withdrawing().unwrap();
        assert_eq!(withdrawing.collected, amount);
        assert_eq!(withdrawing.remaining, 0);
        assert_eq!(withdrawing.collected + withdrawing.remaining, amount);
    }

    #[kani::proof]
    fn settled_head_must_match_head_identity_before_payout() {
        let op_id = 32;
        let withdrawing_state = withdrawal_step_callback(
            start_withdrawal(
                OpState::Idle,
                WithdrawalRequest {
                    op_id,
                    request_id: 0,
                    amount: bounded_amount(),
                    receiver: RECEIVER,
                    owner: OWNER,
                    escrow_shares: nonzero_bounded_amount(),
                },
            )
            .unwrap()
            .new_state,
            op_id,
            bounded_amount(),
        )
        .unwrap()
        .new_state;
        let withdrawing = withdrawing_state.as_withdrawing().unwrap();
        let min_assets_out = bounded_amount();
        let queued = PendingWithdrawal::new(
            OWNER,
            RECEIVER,
            withdrawing.escrow_shares,
            min_assets_out,
            TimestampNs::ZERO,
            EpochId::FIRST_SETTLEMENT,
        )
        .unwrap();
        let mut vault = VaultState::with_initial(
            bounded_amount(),
            bounded_amount(),
            bounded_amount(),
            0,
            TimestampNs::ZERO,
        );
        vault.op_state = withdrawing_state.clone();
        assert_eq!(
            vault.withdraw_queue.enqueue_withdrawal(queued, 3),
            Ok(withdrawing.request_id)
        );
        let claim = settled_claim(
            vault.withdraw_queue.head().unwrap().1,
            &vault.epoch,
        );
        let baseline = vault.clone();
        let result = withdrawal_collected(withdrawing_state.clone(), &vault, op_id, min_assets_out);
        let result = match result {
            Ok(result) => result,
            Err(_) => {
                assert!(vault == baseline);
                return;
            }
        };
        assert!(result.new_state.is_payout());
        let payout = result.new_state.as_payout().unwrap();
        assert_eq!(payout.amount, claim.unwrap());
        let (_, head) = vault.withdraw_queue.head().unwrap();
        assert_eq!(payout.escrow_shares, head.escrow_shares);
        assert_eq!(payout.burn_shares, head.escrow_shares);
    }

    #[kani::proof]
    fn unsettled_head_is_rejected_before_payout_construction() {
        let op_id = 33;
        let withdrawing_state = withdrawal_step_callback(
            start_withdrawal(
                OpState::Idle,
                WithdrawalRequest {
                    op_id,
                    request_id: 0,
                    amount: nonzero_bounded_amount(),
                    receiver: RECEIVER,
                    owner: OWNER,
                    escrow_shares: nonzero_bounded_amount(),
                },
            )
            .unwrap()
            .new_state,
            op_id,
            bounded_amount(),
        )
        .unwrap()
        .new_state;
        let withdrawing = withdrawing_state.as_withdrawing().unwrap();
        let queued = PendingWithdrawal::migrated_legacy(
            OWNER,
            RECEIVER,
            withdrawing.escrow_shares,
            TimestampNs::ZERO,
        )
        .unwrap();
        let mut vault = VaultState::with_initial(
            bounded_amount(),
            bounded_amount(),
            bounded_amount(),
            0,
            TimestampNs::ZERO,
        );
        vault.op_state = withdrawing_state.clone();
        assert_eq!(
            vault.withdraw_queue.enqueue_withdrawal(queued, 3),
            Ok(withdrawing.request_id)
        );
        let baseline = vault.clone();
        assert!(withdrawal_collected(withdrawing_state.clone(), &vault, op_id, bounded_amount()).is_err());
        assert!(withdrawal_settled(withdrawing_state.clone(), &vault, op_id, bounded_amount()).is_err());
        assert!(withdrawal_collected(withdrawing_state.clone(), &vault, 34, bounded_amount()).is_err());
        assert!(payout_complete(
            withdrawing_state.clone(),
            &vault,
            true,
            op_id,
            RECEIVER,
            bounded_amount()
        )
        .is_err());
        assert!(payout_complete(
            withdrawing_state,
            &vault,
            false,
            op_id,
            RECEIVER,
            bounded_amount()
        )
        .is_err());
        assert!(vault == baseline);
    }

    #[kani::proof]
    fn payout_construction_output_is_the_bound_snapshot_claim() {
        let op_id = 36;
        let escrow_shares = nonzero_bounded_amount();
        let withdrawing_state = withdrawal_step_callback(
            start_withdrawal(
                OpState::Idle,
                WithdrawalRequest {
                    op_id,
                    request_id: 0,
                    amount: bounded_amount(),
                    receiver: RECEIVER,
                    owner: OWNER,
                    escrow_shares,
                },
            )
            .unwrap()
            .new_state,
            op_id,
            bounded_amount(),
        )
        .unwrap()
        .new_state;
        let withdrawing = withdrawing_state.as_withdrawing().unwrap();
        let queued = PendingWithdrawal::new(
            OWNER,
            RECEIVER,
            withdrawing.escrow_shares,
            0,
            TimestampNs::ZERO,
            EpochId::FIRST_SETTLEMENT,
        )
        .unwrap();
        let mut vault = VaultState::with_initial(
            bounded_amount(),
            bounded_amount(),
            bounded_amount(),
            0,
            TimestampNs::ZERO,
        );
        vault.op_state = withdrawing_state.clone();
        assert_eq!(
            vault.withdraw_queue.enqueue_withdrawal(queued, 3),
            Ok(withdrawing.request_id)
        );
        let baseline = vault.clone();
        assert_eq!(
            vault.withdraw_queue.settled_head_claim(&vault.epoch),
            None,
            "an unsettled queue head cannot source any payout figure"
        );
        match withdrawal_collected(withdrawing_state.clone(), &vault, op_id, 0) {
            Err(_) => assert!(vault == baseline),
            Ok(result) => {
                let payout = result
                    .new_state
                    .as_payout()
                    .expect("unsettled payout construction cannot produce another state");
                assert_eq!(payout.amount, 0, "unsettled construction cannot source an asset amount");
                assert_eq!(payout.escrow_shares, 0);
                assert_eq!(payout.burn_shares, 0);
                assert!(vault == baseline);
            }
        }

        settle_epoch(
            &mut vault,
            bounded_amount(),
            nonzero_bounded_amount(),
        );
        let claim = vault
            .withdraw_queue
            .settled_head_claim(&vault.epoch)
            .expect("settled epoch derives the head claim");
        let collected = withdrawal_collected(withdrawing_state, &vault, op_id, 0)
            .expect("settled head constructs the lawful payout");
        assert!(collected.new_state.is_payout());
        let payout = collected.new_state.as_payout().unwrap();
        let head = vault.withdraw_queue.head().unwrap().1;
        assert_eq!(payout.amount, claim);
        assert_eq!(payout.escrow_shares, head.escrow_shares);
        assert_eq!(payout.burn_shares, head.escrow_shares);
        assert!(vault.withdraw_queue.check_invariants());
        assert_asset_sum(&vault);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn withdrawal_queue_head_identity_must_match_the_queued_request() {
        let mut state = VaultState::with_initial(16, 16, 16, 0, TimestampNs::ZERO);
        let first_id = enqueue_withdrawal(&mut state, OWNER, RECEIVER, 3, 0, TimestampNs::ZERO);

        let (head_id, head) = state.withdraw_queue.head().expect("head must be queued");
        assert_eq!(head_id, first_id);
        assert_address_eq(head.owner, OWNER);
        assert_address_eq(head.receiver, RECEIVER);
        assert_eq!(head.escrow_shares, 3);
        // No other index resolves to this request, so a mismatched caller
        // claim about the head cannot be satisfied by an adjacent entry.
        assert_eq!(state.withdraw_queue.get(first_id + 1), None);
        assert!(state
            .withdraw_queue
            .get(first_id)
            .is_some_and(|w| w.owner != SECOND_OWNER));
        assert!(state
            .withdraw_queue
            .get(first_id)
            .is_some_and(|w| w.receiver != SECOND_RECEIVER));
        assert!(state
            .withdraw_queue
            .get(first_id)
            .is_some_and(|w| w.escrow_shares != 4));
        assert!(state.withdraw_queue.check_invariants_with_max(3));
        assert_eq!(
            state.withdraw_queue.status().total_escrow_shares,
            head.escrow_shares
        );
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn withdrawal_queue_head_cannot_be_replaced_by_a_later_fifo_entry() {
        let mut state = VaultState::with_initial(16, 16, 16, 0, TimestampNs::ZERO);
        let first_id = enqueue_withdrawal(&mut state, OWNER, RECEIVER, 3, 0, TimestampNs::ZERO);
        let second_id = enqueue_withdrawal(
            &mut state,
            SECOND_OWNER,
            SECOND_RECEIVER,
            7,
            0,
            TimestampNs::ZERO,
        );
        let before = state.withdraw_queue.status();

        let (head_id, _) = state.withdraw_queue.head().expect("head must be queued");
        assert_eq!(head_id, first_id);
        assert_eq!(
            state.withdraw_queue.get(second_id).map(|w| w.escrow_shares),
            Some(7)
        );
        // The later entry is queued but is not the head, so it cannot be
        // executed ahead of the earlier request.
        assert_ne!(second_id, head_id);
        assert_eq!(
            state.withdraw_queue.head().map(|(id, _)| id),
            Some(first_id)
        );
        assert_eq!(state.withdraw_queue.status().length, before.length);
        assert_eq!(
            state.withdraw_queue.status().total_escrow_shares,
            before.total_escrow_shares
        );
        assert!(state.withdraw_queue.check_invariants_with_max(3));
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn withdrawal_head_amount_is_only_the_epoch_derived_claim() {
        let mut state = VaultState::with_initial(16, 16, 16, 0, TimestampNs::ZERO);
        let first_id = enqueue_withdrawal(&mut state, OWNER, RECEIVER, 3, 0, TimestampNs::ZERO);

        let (head_id, head) = state.withdraw_queue.head().expect("head must be queued");
        assert_eq!(head_id, first_id);
        assert_address_eq(head.owner, OWNER);
        assert_address_eq(head.receiver, RECEIVER);
        assert_eq!(head.escrow_shares, 3);
        // Before settlement the head has no payout figure at all.
        assert_eq!(settled_claim(head, &state.epoch), None);
        assert_eq!(state.withdraw_queue.settled_head_claim(&state.epoch), None);

        let head_owner = head.owner;
        let head_receiver = head.receiver;
        let head_escrow_shares = head.escrow_shares;
        // NAV 32 over supply 16 makes the floor claim on 3 shares exactly 6.
        settle_epoch(&mut state, 32, 16);
        let claim = state
            .withdraw_queue
            .settled_head_claim(&state.epoch)
            .expect("settled epoch derives the head claim");
        assert_eq!(claim, 6);
        assert_eq!(
            state.withdraw_queue.get(first_id).and_then(|w| settled_claim(w, &state.epoch)),
            Some(claim)
        );

        let request = WithdrawalRequest {
            op_id: 1,
            request_id: first_id,
            amount: claim,
            receiver: head_receiver,
            owner: head_owner,
            escrow_shares: head_escrow_shares,
        };
        let started = start_withdrawal(OpState::Idle, request).unwrap().new_state;
        let withdrawing = started.as_withdrawing().unwrap();
        assert_eq!(withdrawing.request_id, first_id);
        assert_address_eq(withdrawing.owner, OWNER);
        assert_address_eq(withdrawing.receiver, RECEIVER);
        assert_eq!(withdrawing.escrow_shares, 3);
        assert_eq!(withdrawing.remaining, claim);
        assert_eq!(state.withdraw_queue.status().length, 1);
        assert_eq!(state.withdraw_queue.status().total_escrow_shares, 3);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn payout_queue_head_dequeues_once_before_settlement() {
        let mut queue = WithdrawQueue::new();
        let first_id = queue
            .enqueue(
                OWNER,
                RECEIVER,
                3,
                0,
                TimestampNs::ZERO,
                EpochId::FIRST_SETTLEMENT,
                3,
            )
            .unwrap();
        let second_id = queue
            .enqueue(
                SECOND_OWNER,
                SECOND_RECEIVER,
                7,
                0,
                TimestampNs::ZERO,
                EpochId::FIRST_SETTLEMENT,
                3,
            )
            .unwrap();

        let (dequeued_id, dequeued) = queue.dequeue().unwrap();
        assert_eq!(dequeued_id, first_id);
        assert_address_eq(dequeued.owner, OWNER);
        assert_address_eq(dequeued.receiver, RECEIVER);
        assert_eq!(dequeued.escrow_shares, 3);
        assert_eq!(settled_claim(&dequeued, &EpochState::genesis()), None);
        assert_eq!(queue.status().length, 1);
        assert_eq!(queue.status().total_escrow_shares, 7);
        assert_eq!(queue.head().map(|(id, _)| id), Some(second_id));
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn payout_settlement_must_burn_or_refund_every_escrowed_share() {
        let escrow_shares = nonzero_bounded_amount();
        let burn_shares = bounded_amount();
        kani::assume(burn_shares <= escrow_shares);
        let entry = EscrowEntry::new(OWNER, escrow_shares, TimestampNs::ZERO);

        // A successful payout burns the entire escrow and refunds nothing.
        let success = apply_settlement(&entry, &EscrowSettlement::burn_all(escrow_shares))
            .expect("a successful payout must burn every escrowed share");
        assert_eq!(success.burned, escrow_shares);
        assert_eq!(success.refunded, 0);

        // A failed payout refunds the entire escrow and burns nothing.
        let failure = apply_settlement(&entry, &EscrowSettlement::refund_all(escrow_shares))
            .expect("a failed payout must refund every escrowed share");
        assert_eq!(failure.burned, 0);
        assert_eq!(failure.refunded, escrow_shares);

        // Any other outcome must be an exact partition of the escrow.
        let partial = apply_settlement(
            &entry,
            &EscrowSettlement::partial(burn_shares, escrow_shares - burn_shares),
        )
        .expect("a partial payout settlement must conserve escrow");
        assert_eq!(partial.burned + partial.refunded, escrow_shares);

        // Settling more than is escrowed is rejected on both sides.
        assert_eq!(
            apply_settlement(&entry, &EscrowSettlement::burn_all(escrow_shares + 1)),
            None
        );
        assert_eq!(
            apply_settlement(&entry, &EscrowSettlement::refund_all(escrow_shares + 1)),
            None
        );
        assert!(total_burn(&[EscrowSettlement::burn_all(escrow_shares)]) == Some(escrow_shares));
        assert!(
            total_refund(&[EscrowSettlement::burn_all(escrow_shares)]) == Some(0)
        );
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn failed_withdrawal_refunds_all_escrow_without_mutating_assets_or_shares() {
        let idle = nonzero_bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let escrow_shares = nonzero_bounded_amount();
        kani::assume(escrow_shares <= total_shares);

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
            0,
            TimestampNs::ZERO,
        );
        assert!(state.withdraw_queue.check_invariants_with_max(3));
        assert_eq!(
            state.withdraw_queue.status().total_escrow_shares,
            escrow_shares
        );

        let (dequeued_id, dequeued) = state.withdraw_queue.dequeue().unwrap();
        assert_eq!(dequeued_id, request_id);
        assert_address_eq(dequeued.owner, OWNER);
        assert_address_eq(dequeued.receiver, RECEIVER);
        assert_eq!(dequeued.escrow_shares, escrow_shares);
        assert_eq!(state.withdraw_queue.status().length, 0);

        // The unsettled request has no payout figure, so the only lawful
        // settlement is a refund of the entire escrow.
        assert_eq!(settled_claim(&dequeued, &state.epoch), None);
        let entry = EscrowEntry::new(
            dequeued.owner,
            dequeued.escrow_shares,
            dequeued.requested_at_ns,
        );
        let settlement = EscrowSettlement::refund_all(dequeued.escrow_shares);
        let refunded = apply_settlement(&entry, &settlement)
            .expect("cancellation must release every escrowed share");
        assert_eq!(refunded.burned, 0);
        assert_eq!(refunded.refunded, escrow_shares);
        let settlements = [settlement];
        assert_eq!(total_burn(&settlements), Some(0));
        assert_eq!(total_refund(&settlements), Some(escrow_shares));

        assert!(state.op_state.is_idle());
        assert_asset_sum(&state);
        assert_eq!(state.idle_assets, idle);
        assert_eq!(state.external_assets, external);
        assert_eq!(state.total_assets, idle + external);
        assert_eq!(state.total_shares, total_shares);
        assert_eq!(state.withdraw_queue.status().length, 0);
        assert!(state.check_invariant());
    }

    #[cfg(feature = "action-recovery")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn emergency_reset_allocating_restores_remaining_assets_to_idle() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = bounded_amount();
        let remaining = bounded_amount();
        let op_id = 51;

        let mut state = VaultState::with_initial(
            idle + external,
            total_shares,
            idle,
            external,
            TimestampNs::ZERO,
        );
        state.op_state = OpState::Allocating(AllocatingState {
            op_id,
            index: 0,
            remaining,
            plan: allocation_plan(remaining, 0),
        });

        let result = plan_emergency_reset(state).unwrap();

        assert!(result.state.op_state.is_idle());
        assert_eq!(result.state.idle_assets, idle + remaining);
        assert_eq!(result.state.external_assets, external);
        assert_eq!(result.state.total_assets, idle + external + remaining);
        assert_eq!(result.state.total_shares, total_shares);
        assert_eq!(result.state.withdraw_queue.status().length, 0);
        assert!(result.refund_owner.is_none());
        assert_eq!(result.refund_shares, 0);
        assert_eq!(
            result.state.fee_anchor.total_assets,
            result.state.total_assets
        );
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-recovery")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn emergency_reset_withdrawing_restores_collected_assets_and_refunds_escrow() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let remaining = bounded_amount();
        let collected = bounded_amount();
        let escrow_shares = nonzero_bounded_amount();
        let op_id = 52;

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
            0,
            TimestampNs::ZERO,
        );
        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id,
            request_id,
            index: 0,
            remaining,
            collected,
            receiver: RECEIVER,
            owner: OWNER,
            escrow_shares,
        });

        let result = plan_emergency_reset(state).unwrap();

        assert!(result.state.op_state.is_idle());
        assert_eq!(result.state.idle_assets, idle + collected);
        assert_eq!(result.state.external_assets, external);
        assert_eq!(result.state.total_assets, idle + external + collected);
        assert_eq!(result.state.total_shares, total_shares);
        assert_eq!(result.state.withdraw_queue.status().length, 0);
        assert_refund_owner_is_owner(result.refund_owner);
        assert_eq!(result.refund_shares, escrow_shares);
        assert_eq!(
            result.state.fee_anchor.total_assets,
            result.state.total_assets
        );
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-recovery")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn emergency_reset_payout_restores_payout_assets_and_refunds_escrow() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let amount = bounded_amount();
        let escrow_shares = nonzero_bounded_amount();
        let burn_shares = bounded_amount();
        let op_id = 53;
        kani::assume(burn_shares <= escrow_shares);

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
            0,
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

        let result = plan_emergency_reset(state).unwrap();

        assert!(result.state.op_state.is_idle());
        assert_eq!(result.state.idle_assets, idle + amount);
        assert_eq!(result.state.external_assets, external);
        assert_eq!(result.state.total_assets, idle + external + amount);
        assert_eq!(result.state.total_shares, total_shares);
        assert_eq!(result.state.withdraw_queue.status().length, 0);
        assert_refund_owner_is_owner(result.refund_owner);
        assert_eq!(result.refund_shares, escrow_shares);
        assert_eq!(
            result.state.fee_anchor.total_assets,
            result.state.total_assets
        );
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-recovery")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn emergency_reset_refreshing_returns_idle_without_accounting_mutation() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = bounded_amount();
        let op_id = 54;

        let mut state = VaultState::with_initial(
            idle + external,
            total_shares,
            idle,
            external,
            TimestampNs::ZERO,
        );
        let before = state.clone();
        state.op_state = OpState::Refreshing(RefreshingState {
            op_id,
            index: 1,
            plan: vec![7, 8],
        });

        let result = plan_emergency_reset(state).unwrap();

        assert!(result.state.op_state.is_idle());
        assert_eq!(result.state.idle_assets, before.idle_assets);
        assert_eq!(result.state.external_assets, before.external_assets);
        assert_eq!(result.state.total_assets, before.total_assets);
        assert_eq!(result.state.total_shares, before.total_shares);
        assert_eq!(
            result.state.withdraw_queue.status().length,
            before.withdraw_queue.status().length
        );
        assert!(result.refund_owner.is_none());
        assert_eq!(result.refund_shares, 0);
        assert_eq!(
            result.state.fee_anchor.total_assets,
            result.state.total_assets
        );
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-sync-external")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn sync_external_assets_allocating_only_mutates_external_and_total_assets() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let synced_external = bounded_amount();
        let shares = bounded_amount();
        let op_id = 61;

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id,
            index: 1,
            remaining: 2,
            plan: allocation_plan(1, 1),
        });

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
        assert_eq!(result.state.next_op_id, before_next_op_id);
        assert_eq!(
            result.state.fee_anchor.total_assets,
            before_fee_anchor_total_assets
        );
        assert!(result.state.fee_anchor.timestamp_ns == before_fee_anchor_timestamp);
        if let OpState::Allocating(alloc) = &result.state.op_state {
            assert_eq!(alloc.op_id, op_id);
            assert_eq!(alloc.index, 1);
            assert_eq!(alloc.remaining, 2);
        } else {
            panic!("sync must preserve allocating operation");
        }
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-sync-external")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn sync_external_assets_withdrawing_preserves_share_supply_queue_and_actor_fields() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let synced_external = bounded_amount();
        let shares = bounded_amount();
        let op_id = 62;

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id,
            request_id: 7,
            index: 1,
            remaining: 2,
            collected: 2,
            receiver: RECEIVER,
            owner: OWNER,
            escrow_shares: 3,
        });

        let before_queue = state.withdraw_queue.status();
        let before_next_op_id = state.next_op_id;
        let result = apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id, TimestampNs::ZERO),
        )
        .unwrap();

        assert_eq!(result.state.idle_assets, idle);
        assert_eq!(result.state.external_assets, synced_external);
        assert_eq!(result.state.total_assets, idle + synced_external);
        assert_eq!(result.state.total_shares, shares);
        assert_eq!(
            result.state.withdraw_queue.status().length,
            before_queue.length
        );
        assert_eq!(
            result.state.withdraw_queue.status().total_escrow_shares,
            before_queue.total_escrow_shares
        );
        assert_eq!(result.state.next_op_id, before_next_op_id);
        if let OpState::Withdrawing(withdraw) = &result.state.op_state {
            assert_eq!(withdraw.op_id, op_id);
            assert_eq!(withdraw.request_id, 7);
            assert_eq!(withdraw.index, 1);
            assert_eq!(withdraw.remaining, 2);
            assert_eq!(withdraw.collected, 2);
            assert_address_eq(withdraw.owner, OWNER);
            assert_address_eq(withdraw.receiver, RECEIVER);
            assert_eq!(withdraw.escrow_shares, 3);
        } else {
            panic!("sync must preserve withdrawing operation");
        }
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-sync-external")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn sync_external_assets_refreshing_only_mutates_external_and_total_assets() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let synced_external = bounded_amount();
        let shares = bounded_amount();
        let op_id = 63;

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        state.op_state = OpState::Refreshing(RefreshingState {
            op_id,
            index: 1,
            plan: vec![7, 8],
        });

        let before_queue = state.withdraw_queue.status();
        let before_next_op_id = state.next_op_id;
        let result = apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id, TimestampNs::ZERO),
        )
        .unwrap();

        assert_eq!(result.state.idle_assets, idle);
        assert_eq!(result.state.external_assets, synced_external);
        assert_eq!(result.state.total_assets, idle + synced_external);
        assert_eq!(result.state.total_shares, shares);
        assert_eq!(
            result.state.withdraw_queue.status().length,
            before_queue.length
        );
        assert_eq!(
            result.state.withdraw_queue.status().total_escrow_shares,
            before_queue.total_escrow_shares
        );
        assert_eq!(result.state.next_op_id, before_next_op_id);
        if let OpState::Refreshing(refresh) = &result.state.op_state {
            assert_eq!(refresh.op_id, op_id);
            assert_eq!(refresh.index, 1);
        } else {
            panic!("sync must preserve refreshing operation");
        }
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-sync-external")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn sync_external_assets_rejects_wrong_op_id_and_disallowed_states() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let synced_external = bounded_amount();
        let shares = bounded_amount();
        let op_id = 64;

        let mut allocating =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        allocating.op_state = OpState::Allocating(AllocatingState {
            op_id,
            index: 1,
            remaining: 2,
            plan: allocation_plan(1, 1),
        });
        assert!(apply_action(
            allocating,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id + 1, TimestampNs::ZERO),
        )
        .is_err());

        let idle_state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        assert!(apply_action(
            idle_state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id, TimestampNs::ZERO),
        )
        .is_err());

        let mut payout_state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        payout_state.op_state = OpState::Payout(PayoutState {
            op_id,
            request_id: 0,
            receiver: RECEIVER,
            amount: 1,
            owner: OWNER,
            escrow_shares: 1,
            burn_shares: 1,
        });
        assert!(apply_action(
            payout_state,
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

        assert_eq!(result.state.idle_assets, before.idle_assets);
        assert_eq!(result.state.external_assets, before.external_assets);
        assert_eq!(result.state.total_assets, before.total_assets);
        assert_eq!(result.effects.len(), 1);
        assert_emit_event_effect(&result.effects[0]);
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
        assert_eq!(result.state.next_op_id, before.next_op_id);
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-refresh-fees")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn refresh_fees_active_rates_only_mint_fee_shares_and_update_anchor() {
        let idle = 100u128;
        let external = 0u128;
        let shares = 100u128;
        let anchor_assets = 0u128;
        let now = TimestampNs::from_nanos(1);

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        state.fee_anchor = FeeAccrualAnchor::new(anchor_assets, TimestampNs::ZERO);
        let before = state.clone();
        let before_queue = before.withdraw_queue.status();

        let result = apply_action(
            state,
            &active_fee_config(),
            None,
            &SELF,
            KernelAction::refresh_fees(now),
        )
        .unwrap();

        assert_eq!(result.effects.len(), 2);
        let minted = assert_mint_shares_effect(&result.effects[0]);
        assert_emit_event_effect(&result.effects[1]);

        assert!(minted > 0);
        assert_eq!(result.state.idle_assets, before.idle_assets);
        assert_eq!(result.state.external_assets, before.external_assets);
        assert_eq!(result.state.total_assets, before.total_assets);
        assert!(result.state.total_shares >= before.total_shares);
        assert_eq!(result.state.total_shares, before.total_shares + minted);
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
        assert_eq!(result.state.next_op_id, before.next_op_id);
        assert_asset_sum(&result.state);
    }
}

#[cfg(all(kani, not(feature = "action-epoch-settlement")))]
mod legacy_kani_proofs {
    use alloc::{vec, vec::Vec};

    use super::*;
    #[cfg(feature = "action-recovery")]
    use crate::actions::plan_emergency_reset;
    use crate::actions::{
        apply_payout_settlement_legacy, apply_withdrawal_request_plan_legacy, pending_withdrawal_head,
        plan_payout_settlement_legacy, validate_queue_head, withdrawal_request_from_head,
        WithdrawalRequestPlan,
    };
    use crate::effects::KernelEffect;

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

    #[cfg(feature = "action-refresh-fees")]
    fn active_fee_config() -> VaultConfig {
        VaultConfig {
            fees: FeesSpec::new(
                FeeSlot::new(Wad::one() / 10, Address([0x66; 32])),
                FeeSlot::zero(),
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
        assert_eq!(left.0[0], right.0[0]);
        assert_eq!(left.0[1], right.0[1]);
        assert_eq!(left.0[2], right.0[2]);
        assert_eq!(left.0[3], right.0[3]);
        assert_eq!(left.0[4], right.0[4]);
        assert_eq!(left.0[5], right.0[5]);
        assert_eq!(left.0[6], right.0[6]);
        assert_eq!(left.0[7], right.0[7]);
        assert_eq!(left.0[8], right.0[8]);
        assert_eq!(left.0[9], right.0[9]);
        assert_eq!(left.0[10], right.0[10]);
        assert_eq!(left.0[11], right.0[11]);
        assert_eq!(left.0[12], right.0[12]);
        assert_eq!(left.0[13], right.0[13]);
        assert_eq!(left.0[14], right.0[14]);
        assert_eq!(left.0[15], right.0[15]);
        assert_eq!(left.0[16], right.0[16]);
        assert_eq!(left.0[17], right.0[17]);
        assert_eq!(left.0[18], right.0[18]);
        assert_eq!(left.0[19], right.0[19]);
        assert_eq!(left.0[20], right.0[20]);
        assert_eq!(left.0[21], right.0[21]);
        assert_eq!(left.0[22], right.0[22]);
        assert_eq!(left.0[23], right.0[23]);
        assert_eq!(left.0[24], right.0[24]);
        assert_eq!(left.0[25], right.0[25]);
        assert_eq!(left.0[26], right.0[26]);
        assert_eq!(left.0[27], right.0[27]);
        assert_eq!(left.0[28], right.0[28]);
        assert_eq!(left.0[29], right.0[29]);
        assert_eq!(left.0[30], right.0[30]);
        assert_eq!(left.0[31], right.0[31]);
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

    fn assert_transfer_shares_effect(
        effect: &KernelEffect,
        expected_from: Address,
        expected_to: Address,
        expected_shares: u128,
    ) {
        match effect {
            KernelEffect::TransferShares { from, to, shares } => {
                assert_address_eq(*from, expected_from);
                assert_address_eq(*to, expected_to);
                assert_eq!(*shares, expected_shares);
            }
            _ => panic!("expected transfer shares effect"),
        }
    }

    fn assert_emit_event_effect(effect: &KernelEffect) {
        match effect {
            KernelEffect::EmitEvent { .. } => {}
            _ => panic!("expected emit event effect"),
        }
    }

    #[cfg(feature = "action-refresh-fees")]
    fn assert_mint_shares_effect(effect: &KernelEffect) -> u128 {
        match effect {
            KernelEffect::MintShares { shares, .. } => {
                assert!(*shares > 0);
                *shares
            }
            _ => panic!("refresh fees must not move assets or non-fee shares"),
        }
    }

    fn assert_refund_owner_is_owner(refund_owner: Option<Address>) {
        match refund_owner {
            Some(owner) => assert_address_eq(owner, OWNER),
            None => panic!("expected refund owner"),
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

    #[kani::proof]
    #[kani::unwind(40)]
    fn withdrawal_request_plan_preserves_accounting_and_enqueues_exact_escrow() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let shares = nonzero_bounded_amount();
        let expected_assets = bounded_amount();
        kani::assume(idle + external <= MAX_AMOUNT);
        let config = zero_fee_config();
        let state = VaultState::with_initial(
            idle + external,
            total_shares,
            idle,
            external,
            TimestampNs::ZERO,
        );
        let before = state.clone();
        let plan = WithdrawalRequestPlan {
            owner: RECEIVER,
            receiver: OWNER,
            shares,
            expected_assets,
        };

        let requested =
            apply_withdrawal_request_plan_legacy(state, &config, &SELF, plan, TimestampNs::ZERO).unwrap();

        assert!(requested.state.op_state.is_idle());
        assert_eq!(requested.state.idle_assets, before.idle_assets);
        assert_eq!(requested.state.external_assets, before.external_assets);
        assert_eq!(requested.state.total_assets, before.total_assets);
        assert_eq!(requested.state.total_shares, before.total_shares);
        assert_eq!(requested.state.next_op_id, before.next_op_id);
        assert_eq!(requested.state.withdraw_queue.status().length, 1);
        assert_eq!(
            requested.state.withdraw_queue.status().total_escrow_shares,
            shares
        );
        assert_eq!(
            requested
                .state
                .withdraw_queue
                .status()
                .total_expected_assets,
            expected_assets
        );
        let (request_id, head) = requested.state.withdraw_queue.head().unwrap();
        assert_eq!(request_id, 0);
        assert_address_eq(head.owner, RECEIVER);
        assert_address_eq(head.receiver, OWNER);
        assert_eq!(head.escrow_shares, shares);
        assert_eq!(head.expected_assets, expected_assets);
        assert_eq!(requested.effects.len(), 2);
        assert_transfer_shares_effect(&requested.effects[0], RECEIVER, SELF, shares);
        assert_emit_event_effect(&requested.effects[1]);
        assert_asset_sum(&requested.state);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn post_deposit_request_withdraw_preserves_accounting_and_escrows_previewed_shares() {
        let deposited_assets = nonzero_bounded_amount();
        let minted_shares = deposited_assets;
        let config = zero_fee_config();
        let post_deposit = VaultState::with_initial(
            deposited_assets,
            minted_shares,
            deposited_assets,
            0,
            TimestampNs::from_nanos(1),
        );
        let post_deposit_idle_assets = post_deposit.idle_assets;
        let post_deposit_external_assets = post_deposit.external_assets;
        let post_deposit_total_assets = post_deposit.total_assets;
        let post_deposit_total_shares = post_deposit.total_shares;
        let post_deposit_next_op_id = post_deposit.next_op_id;
        let expected_assets = deposited_assets;
        let request_plan = WithdrawalRequestPlan {
            owner: RECEIVER,
            receiver: OWNER,
            shares: minted_shares,
            expected_assets,
        };

        let requested = apply_withdrawal_request_plan_legacy(
            post_deposit,
            &config,
            &SELF,
            request_plan,
            TimestampNs::from_nanos(2),
        )
        .unwrap();

        assert!(requested.state.op_state.is_idle());
        assert_eq!(requested.state.idle_assets, post_deposit_idle_assets);
        assert_eq!(
            requested.state.external_assets,
            post_deposit_external_assets
        );
        assert_eq!(requested.state.total_assets, post_deposit_total_assets);
        assert_eq!(requested.state.total_shares, post_deposit_total_shares);
        assert_eq!(requested.state.next_op_id, post_deposit_next_op_id);
        assert_eq!(requested.state.withdraw_queue.status().length, 1);
        assert_eq!(
            requested.state.withdraw_queue.status().total_escrow_shares,
            minted_shares
        );
        assert_eq!(
            requested
                .state
                .withdraw_queue
                .status()
                .total_expected_assets,
            expected_assets
        );
        let (request_id, head) = requested.state.withdraw_queue.head().unwrap();
        assert_eq!(request_id, 0);
        assert_address_eq(head.owner, RECEIVER);
        assert_address_eq(head.receiver, OWNER);
        assert_eq!(head.escrow_shares, minted_shares);
        assert_eq!(head.expected_assets, expected_assets);
        assert_eq!(requested.effects.len(), 2);
        assert_transfer_shares_effect(&requested.effects[0], RECEIVER, SELF, minted_shares);
        assert_emit_event_effect(&requested.effects[1]);
        assert_asset_sum(&requested.state);
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

    #[kani::proof]
    #[kani::unwind(40)]
    fn withdrawal_queue_head_validation_requires_exact_identity_fields() {
        let mut state = VaultState::with_initial(16, 16, 16, 0, TimestampNs::ZERO);
        let first_id = enqueue_withdrawal(&mut state, OWNER, RECEIVER, 3, 5, TimestampNs::ZERO);

        assert!(
            validate_queue_head(&state.withdraw_queue, first_id, &OWNER, &RECEIVER, 3,).is_ok()
        );
        assert!(
            validate_queue_head(&state.withdraw_queue, first_id + 1, &OWNER, &RECEIVER, 3,)
                .is_err()
        );
        assert!(
            validate_queue_head(&state.withdraw_queue, first_id, &SECOND_OWNER, &RECEIVER, 3,)
                .is_err()
        );
        assert!(
            validate_queue_head(&state.withdraw_queue, first_id, &OWNER, &SECOND_RECEIVER, 3,)
                .is_err()
        );
        assert!(
            validate_queue_head(&state.withdraw_queue, first_id, &OWNER, &RECEIVER, 4,).is_err()
        );
        assert_eq!(
            state.withdraw_queue.head().map(|(id, _)| id),
            Some(first_id)
        );
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn withdrawal_queue_head_validation_rejects_later_fifo_entry() {
        let mut state = VaultState::with_initial(16, 16, 16, 0, TimestampNs::ZERO);
        let first_id = enqueue_withdrawal(&mut state, OWNER, RECEIVER, 3, 5, TimestampNs::ZERO);
        let second_id = enqueue_withdrawal(
            &mut state,
            SECOND_OWNER,
            SECOND_RECEIVER,
            7,
            11,
            TimestampNs::ZERO,
        );
        let before = state.withdraw_queue.status();

        assert!(
            validate_queue_head(&state.withdraw_queue, first_id, &OWNER, &RECEIVER, 3,).is_ok()
        );
        assert!(validate_queue_head(
            &state.withdraw_queue,
            second_id,
            &SECOND_OWNER,
            &SECOND_RECEIVER,
            7,
        )
        .is_err());
        assert_eq!(
            state.withdraw_queue.head().map(|(id, _)| id),
            Some(first_id)
        );
        assert_eq!(state.withdraw_queue.status().length, before.length);
        assert_eq!(
            state.withdraw_queue.status().total_escrow_shares,
            before.total_escrow_shares
        );
        assert_eq!(
            state.withdraw_queue.status().total_expected_assets,
            before.total_expected_assets
        );
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn withdrawal_fifo_head_maps_to_started_withdrawal_request() {
        let mut state = VaultState::with_initial(16, 16, 16, 0, TimestampNs::ZERO);
        let first_id = enqueue_withdrawal(&mut state, OWNER, RECEIVER, 3, 5, TimestampNs::ZERO);

        let head = pending_withdrawal_head(&state).unwrap();
        assert_eq!(head.id, first_id);
        assert_address_eq(head.owner, OWNER);
        assert_address_eq(head.receiver, RECEIVER);
        assert_eq!(head.escrow_shares, 3);
        assert_eq!(head.expected_assets, 5);

        let request = withdrawal_request_from_head(&mut state, head);
        assert_eq!(request.request_id, first_id);
        assert_address_eq(request.owner, OWNER);
        assert_address_eq(request.receiver, RECEIVER);
        assert_eq!(request.escrow_shares, 3);
        assert_eq!(request.amount, 5);
        assert_eq!(state.withdraw_queue.status().length, 1);

        let started = start_withdrawal(OpState::Idle, request).unwrap().new_state;
        let withdrawing = started.as_withdrawing().unwrap();
        assert_eq!(withdrawing.request_id, first_id);
        assert_address_eq(withdrawing.owner, OWNER);
        assert_address_eq(withdrawing.receiver, RECEIVER);
        assert_eq!(withdrawing.escrow_shares, 3);
        assert_eq!(withdrawing.remaining, 5);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn payout_queue_head_dequeues_once_before_settlement() {
        let mut queue = WithdrawQueue::new();
        let first_id = queue
            .enqueue(OWNER, RECEIVER, 3, 5, TimestampNs::ZERO, 3)
            .unwrap();
        let second_id = queue
            .enqueue(SECOND_OWNER, SECOND_RECEIVER, 7, 11, TimestampNs::ZERO, 3)
            .unwrap();

        let (dequeued_id, dequeued) = queue.dequeue().unwrap();
        assert_eq!(dequeued_id, first_id);
        assert_address_eq(dequeued.owner, OWNER);
        assert_address_eq(dequeued.receiver, RECEIVER);
        assert_eq!(dequeued.escrow_shares, 3);
        assert_eq!(dequeued.expected_assets, 5);
        assert_eq!(queue.status().length, 1);
        assert_eq!(queue.status().total_escrow_shares, 7);
        assert_eq!(queue.status().total_expected_assets, 11);
        assert_eq!(queue.head().map(|(id, _)| id), Some(second_id));
    }

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
        let payout = PayoutState {
            op_id,
            request_id,
            receiver: RECEIVER,
            amount,
            owner: OWNER,
            escrow_shares,
            burn_shares,
        };

        assert!(validate_queue_head(
            &state.withdraw_queue,
            payout.request_id,
            &payout.owner,
            &payout.receiver,
            payout.escrow_shares,
        )
        .is_ok());
        let (dequeued_id, dequeued) = state.withdraw_queue.dequeue().unwrap();
        assert_eq!(dequeued_id, request_id);
        assert_address_eq(dequeued.owner, OWNER);
        assert_address_eq(dequeued.receiver, RECEIVER);
        assert_eq!(dequeued.escrow_shares, escrow_shares);
        assert_eq!(state.withdraw_queue.status().length, 0);

        let settlement = plan_payout_settlement_legacy(&payout, PayoutOutcome::Success).unwrap();
        let mut effects = Vec::new();
        apply_payout_settlement_legacy(&mut state, &payout, settlement, SELF, &mut effects).unwrap();

        assert!(state.op_state.is_idle());
        assert_asset_sum(&state);
        assert!(settlement.success);
        assert_eq!(settlement.burn_shares, burn_shares);
        assert_eq!(settlement.refund_shares, escrow_shares - burn_shares);
        assert_eq!(
            settlement.burn_shares + settlement.refund_shares,
            escrow_shares
        );
        assert_eq!(settlement.completed_amount, amount);
        assert_eq!(state.idle_assets, idle - amount);
        assert_eq!(state.external_assets, external);
        assert_eq!(state.total_assets, idle + external - amount);
        assert_eq!(state.total_shares, total_shares - burn_shares);
        assert_eq!(state.withdraw_queue.status().length, 0);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn payout_failure_settlement_refunds_without_mutating_assets_or_shares_and_dequeues_head_once()
    {
        let idle = nonzero_bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let escrow_shares = nonzero_bounded_amount();
        let burn_shares = bounded_amount();
        let amount = bounded_amount();
        kani::assume(burn_shares <= escrow_shares);
        kani::assume(amount <= idle);

        let op_id = 42;
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
        let payout = PayoutState {
            op_id,
            request_id,
            receiver: RECEIVER,
            amount,
            owner: OWNER,
            escrow_shares,
            burn_shares,
        };

        assert!(validate_queue_head(
            &state.withdraw_queue,
            payout.request_id,
            &payout.owner,
            &payout.receiver,
            payout.escrow_shares,
        )
        .is_ok());
        let (dequeued_id, dequeued) = state.withdraw_queue.dequeue().unwrap();
        assert_eq!(dequeued_id, request_id);
        assert_address_eq(dequeued.owner, OWNER);
        assert_address_eq(dequeued.receiver, RECEIVER);
        assert_eq!(dequeued.escrow_shares, escrow_shares);
        assert_eq!(state.withdraw_queue.status().length, 0);

        let settlement = plan_payout_settlement_legacy(&payout, PayoutOutcome::Failure).unwrap();
        let mut effects = Vec::new();
        apply_payout_settlement_legacy(&mut state, &payout, settlement, SELF, &mut effects).unwrap();

        assert!(state.op_state.is_idle());
        assert_asset_sum(&state);
        assert!(!settlement.success);
        assert_eq!(settlement.burn_shares, 0);
        assert_eq!(settlement.refund_shares, escrow_shares);
        assert_eq!(settlement.completed_amount, 0);
        assert_eq!(state.idle_assets, idle);
        assert_eq!(state.external_assets, external);
        assert_eq!(state.total_assets, idle + external);
        assert_eq!(state.total_shares, total_shares);
    }

    #[cfg(feature = "action-recovery")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn emergency_reset_allocating_restores_remaining_assets_to_idle() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = bounded_amount();
        let remaining = bounded_amount();
        let op_id = 51;

        let mut state = VaultState::with_initial(
            idle + external,
            total_shares,
            idle,
            external,
            TimestampNs::ZERO,
        );
        state.op_state = OpState::Allocating(AllocatingState {
            op_id,
            index: 0,
            remaining,
            plan: allocation_plan(remaining, 0),
        });

        let result = plan_emergency_reset(state).unwrap();

        assert!(result.state.op_state.is_idle());
        assert_eq!(result.state.idle_assets, idle + remaining);
        assert_eq!(result.state.external_assets, external);
        assert_eq!(result.state.total_assets, idle + external + remaining);
        assert_eq!(result.state.total_shares, total_shares);
        assert_eq!(result.state.withdraw_queue.status().length, 0);
        assert!(result.refund_owner.is_none());
        assert_eq!(result.refund_shares, 0);
        assert_eq!(
            result.state.fee_anchor.total_assets,
            result.state.total_assets
        );
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-recovery")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn emergency_reset_withdrawing_restores_collected_assets_and_refunds_escrow() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let remaining = bounded_amount();
        let collected = bounded_amount();
        let escrow_shares = nonzero_bounded_amount();
        let op_id = 52;

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
            collected + remaining,
            TimestampNs::ZERO,
        );
        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id,
            request_id,
            index: 0,
            remaining,
            collected,
            receiver: RECEIVER,
            owner: OWNER,
            escrow_shares,
        });

        let result = plan_emergency_reset(state).unwrap();

        assert!(result.state.op_state.is_idle());
        assert_eq!(result.state.idle_assets, idle + collected);
        assert_eq!(result.state.external_assets, external);
        assert_eq!(result.state.total_assets, idle + external + collected);
        assert_eq!(result.state.total_shares, total_shares);
        assert_eq!(result.state.withdraw_queue.status().length, 0);
        assert_refund_owner_is_owner(result.refund_owner);
        assert_eq!(result.refund_shares, escrow_shares);
        assert_eq!(
            result.state.fee_anchor.total_assets,
            result.state.total_assets
        );
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-recovery")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn emergency_reset_payout_restores_payout_assets_and_refunds_escrow() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = nonzero_bounded_amount();
        let amount = bounded_amount();
        let escrow_shares = nonzero_bounded_amount();
        let burn_shares = bounded_amount();
        let op_id = 53;
        kani::assume(burn_shares <= escrow_shares);

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

        let result = plan_emergency_reset(state).unwrap();

        assert!(result.state.op_state.is_idle());
        assert_eq!(result.state.idle_assets, idle + amount);
        assert_eq!(result.state.external_assets, external);
        assert_eq!(result.state.total_assets, idle + external + amount);
        assert_eq!(result.state.total_shares, total_shares);
        assert_eq!(result.state.withdraw_queue.status().length, 0);
        assert_refund_owner_is_owner(result.refund_owner);
        assert_eq!(result.refund_shares, escrow_shares);
        assert_eq!(
            result.state.fee_anchor.total_assets,
            result.state.total_assets
        );
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-recovery")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn emergency_reset_refreshing_returns_idle_without_accounting_mutation() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let total_shares = bounded_amount();
        let op_id = 54;

        let mut state = VaultState::with_initial(
            idle + external,
            total_shares,
            idle,
            external,
            TimestampNs::ZERO,
        );
        let before = state.clone();
        state.op_state = OpState::Refreshing(RefreshingState {
            op_id,
            index: 1,
            plan: vec![7, 8],
        });

        let result = plan_emergency_reset(state).unwrap();

        assert!(result.state.op_state.is_idle());
        assert_eq!(result.state.idle_assets, before.idle_assets);
        assert_eq!(result.state.external_assets, before.external_assets);
        assert_eq!(result.state.total_assets, before.total_assets);
        assert_eq!(result.state.total_shares, before.total_shares);
        assert_eq!(
            result.state.withdraw_queue.status().length,
            before.withdraw_queue.status().length
        );
        assert!(result.refund_owner.is_none());
        assert_eq!(result.refund_shares, 0);
        assert_eq!(
            result.state.fee_anchor.total_assets,
            result.state.total_assets
        );
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-sync-external")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn sync_external_assets_allocating_only_mutates_external_and_total_assets() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let synced_external = bounded_amount();
        let shares = bounded_amount();
        let op_id = 61;

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id,
            index: 1,
            remaining: 2,
            plan: allocation_plan(1, 1),
        });

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
        if let OpState::Allocating(alloc) = &result.state.op_state {
            assert_eq!(alloc.op_id, op_id);
            assert_eq!(alloc.index, 1);
            assert_eq!(alloc.remaining, 2);
        } else {
            panic!("sync must preserve allocating operation");
        }
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-sync-external")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn sync_external_assets_withdrawing_preserves_share_supply_queue_and_actor_fields() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let synced_external = bounded_amount();
        let shares = bounded_amount();
        let op_id = 62;

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id,
            request_id: 7,
            index: 1,
            remaining: 2,
            collected: 2,
            receiver: RECEIVER,
            owner: OWNER,
            escrow_shares: 3,
        });

        let before_queue = state.withdraw_queue.status();
        let before_next_op_id = state.next_op_id;
        let result = apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id, TimestampNs::ZERO),
        )
        .unwrap();

        assert_eq!(result.state.idle_assets, idle);
        assert_eq!(result.state.external_assets, synced_external);
        assert_eq!(result.state.total_assets, idle + synced_external);
        assert_eq!(result.state.total_shares, shares);
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
        if let OpState::Withdrawing(withdraw) = &result.state.op_state {
            assert_eq!(withdraw.op_id, op_id);
            assert_eq!(withdraw.request_id, 7);
            assert_eq!(withdraw.index, 1);
            assert_eq!(withdraw.remaining, 2);
            assert_eq!(withdraw.collected, 2);
            assert_address_eq(withdraw.owner, OWNER);
            assert_address_eq(withdraw.receiver, RECEIVER);
            assert_eq!(withdraw.escrow_shares, 3);
        } else {
            panic!("sync must preserve withdrawing operation");
        }
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-sync-external")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn sync_external_assets_refreshing_only_mutates_external_and_total_assets() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let synced_external = bounded_amount();
        let shares = bounded_amount();
        let op_id = 63;

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        state.op_state = OpState::Refreshing(RefreshingState {
            op_id,
            index: 1,
            plan: vec![7, 8],
        });

        let before_queue = state.withdraw_queue.status();
        let before_next_op_id = state.next_op_id;
        let result = apply_action(
            state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id, TimestampNs::ZERO),
        )
        .unwrap();

        assert_eq!(result.state.idle_assets, idle);
        assert_eq!(result.state.external_assets, synced_external);
        assert_eq!(result.state.total_assets, idle + synced_external);
        assert_eq!(result.state.total_shares, shares);
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
        if let OpState::Refreshing(refresh) = &result.state.op_state {
            assert_eq!(refresh.op_id, op_id);
            assert_eq!(refresh.index, 1);
        } else {
            panic!("sync must preserve refreshing operation");
        }
        assert_asset_sum(&result.state);
    }

    #[cfg(feature = "action-sync-external")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn sync_external_assets_rejects_wrong_op_id_and_disallowed_states() {
        let idle = bounded_amount();
        let external = bounded_amount();
        let synced_external = bounded_amount();
        let shares = bounded_amount();
        let op_id = 64;

        let mut allocating =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        allocating.op_state = OpState::Allocating(AllocatingState {
            op_id,
            index: 1,
            remaining: 2,
            plan: allocation_plan(1, 1),
        });
        assert!(apply_action(
            allocating,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id + 1, TimestampNs::ZERO),
        )
        .is_err());

        let idle_state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        assert!(apply_action(
            idle_state,
            &zero_fee_config(),
            None,
            &SELF,
            KernelAction::sync_external_assets(synced_external, op_id, TimestampNs::ZERO),
        )
        .is_err());

        let mut payout_state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        payout_state.op_state = OpState::Payout(PayoutState {
            op_id,
            request_id: 0,
            receiver: RECEIVER,
            amount: 1,
            owner: OWNER,
            escrow_shares: 1,
            burn_shares: 1,
        });
        assert!(apply_action(
            payout_state,
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

        assert_eq!(result.state.idle_assets, before.idle_assets);
        assert_eq!(result.state.external_assets, before.external_assets);
        assert_eq!(result.state.total_assets, before.total_assets);
        assert_eq!(result.effects.len(), 1);
        assert_emit_event_effect(&result.effects[0]);
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

    #[cfg(feature = "action-refresh-fees")]
    #[kani::proof]
    #[kani::unwind(8)]
    fn refresh_fees_active_rates_only_mint_fee_shares_and_update_anchor() {
        let idle = 100u128;
        let external = 0u128;
        let shares = 100u128;
        let anchor_assets = 0u128;
        let now = TimestampNs::from_nanos(1);

        let mut state =
            VaultState::with_initial(idle + external, shares, idle, external, TimestampNs::ZERO);
        state.fee_anchor = FeeAccrualAnchor::new(anchor_assets, TimestampNs::ZERO);
        let before = state.clone();
        let before_queue = before.withdraw_queue.status();

        let result = apply_action(
            state,
            &active_fee_config(),
            None,
            &SELF,
            KernelAction::refresh_fees(now),
        )
        .unwrap();

        assert_eq!(result.effects.len(), 2);
        let minted = assert_mint_shares_effect(&result.effects[0]);
        assert_emit_event_effect(&result.effects[1]);

        assert!(minted > 0);
        assert_eq!(result.state.idle_assets, before.idle_assets);
        assert_eq!(result.state.external_assets, before.external_assets);
        assert_eq!(result.state.total_assets, before.total_assets);
        assert!(result.state.total_shares >= before.total_shares);
        assert_eq!(result.state.total_shares, before.total_shares + minted);
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