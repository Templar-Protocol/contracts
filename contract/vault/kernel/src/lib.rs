#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod effects;
pub mod error;
pub mod fee;
pub mod guardrails;
#[cfg(kani)]
pub mod kani;
pub mod math;
pub mod restrictions;
pub mod state;
pub mod transitions;
pub mod types;

// Re-exports for convenience
pub use fee::{Fee, FeeSlot, Fees, FeesSpec};
pub use math::number::Number;
pub use math::wad::{
    compute_fee_shares, compute_fee_shares_from_assets, mul_div_ceil, mul_div_floor, mul_wad_floor,
    Wad, MAX_FEE_WAD, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD,
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
