#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod actions;
pub mod address_book;
pub mod effects;
pub mod error;
pub mod fee;
#[cfg(kani)]
pub mod kani;
pub mod math;
pub mod restrictions;
pub mod state;
#[doc(hidden)]
pub mod test_utils;
pub mod transitions;
pub mod types;

// Re-exports for convenience
pub use actions::{
    apply_action, convert_to_assets, convert_to_assets_ceil, convert_to_shares,
    convert_to_shares_ceil, effective_totals, preview_deposit_shares, preview_withdraw_assets,
    KernelAction, KernelResult, PayoutOutcome,
};
pub use address_book::AddressBook;
pub use fee::{Fee, FeeSlot, Fees, FeesSpec};
pub use math::number::Number;
pub use math::wad::{
    compute_fee_shares, compute_fee_shares_from_assets, compute_management_fee_shares,
    mul_div_ceil, mul_div_floor, mul_wad_floor, total_assets_for_fee_accrual, Wad, MAX_FEE_WAD,
    MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD, YEAR_NS,
};
pub use restrictions::Restrictions;
pub use state::escrow::{
    apply_settlement, can_apply_settlement, compute_escrow_stats, find_by_owner, is_stale,
    settle_full_burn, settle_full_refund, settle_proportional, total_burn, total_refund,
    EscrowEntry, EscrowSettlement, EscrowStats, SettlementResult,
};
pub use state::op_state::{
    AllocatingState, IdleState, OpState, PayoutState, RefreshingState, TargetId, WithdrawingState,
};
pub use state::queue::{
    can_enqueue, can_partially_satisfy, can_satisfy_withdrawal, compute_full_withdrawal,
    compute_partial_withdrawal, compute_queue_status, compute_settlement,
    compute_settlement_by_price, count_satisfiable, find_request_status, is_past_cooldown,
    is_valid_withdrawal_amount, PendingWithdrawal, QueueError, QueueStatus, WithdrawQueue,
    WithdrawalRequestStatus, WithdrawalResult, DEFAULT_COOLDOWN_NS, MAX_PENDING, MAX_QUEUE_LENGTH,
    MIN_WITHDRAWAL_ASSETS,
};
pub use state::vault::{FeeAccrualAnchor, VaultConfig, VaultState};
pub use transitions::{
    allocation_step_callback, complete_allocation, complete_refresh, payout_complete,
    refresh_step_callback, start_allocation, start_refresh, start_withdrawal, stop_withdrawal,
    withdrawal_collected, withdrawal_step_callback, TransitionError, TransitionRes,
    TransitionResult, WithdrawalRequest,
};
pub use types::{ActualIdx, Address, AssetId, ExpectedIdx, KernelVersion, TimestampNs};
