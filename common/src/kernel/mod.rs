//! Chain-agnostic kernel types for the Templar vault.
//!
//! This module re-exports types from the `templar-vault-kernel` crate,
//! providing the foundation for dual-chain deployment.

pub use templar_vault_kernel::*;

// Explicit re-exports for backward compatibility
pub use templar_vault_kernel::{
    fee::{Fee, Fees},
    math::{
        number::{Number, WIDE},
        wad::{
            compute_fee_shares, compute_fee_shares_from_assets, mul_div_ceil, mul_div_floor,
            mul_wad_floor, Wad, MAX_FEE_WAD, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD,
        },
    },
    state::{
        op_state::{
            AllocatingState, IdleState, OpState, PayoutState, RefreshingState, TargetId,
            WithdrawingState,
        },
        queue::{
            can_enqueue, can_partially_satisfy, can_satisfy_withdrawal, compute_full_withdrawal,
            compute_partial_withdrawal, compute_queue_status, compute_settlement,
            compute_settlement_by_price, count_satisfiable, find_request_status,
            is_past_cooldown, is_valid_withdrawal_amount, PendingWithdrawal, QueueStatus,
            WithdrawalRequestStatus, WithdrawalResult, DEFAULT_COOLDOWN_NS, MAX_QUEUE_LENGTH,
            MIN_WITHDRAWAL_ASSETS,
        },
    },
    Restrictions,
};

pub mod share_math {
    //! Re-export of kernel math types for backward compatibility.
    pub use templar_vault_kernel::math::{
        number::{Number, WIDE},
        wad::{
            compute_fee_shares, compute_fee_shares_from_assets, mul_div_ceil, mul_div_floor,
            mul_wad_floor, Wad, MAX_FEE_WAD, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD,
        },
    };
}

pub mod types {
    //! Re-export of kernel types for backward compatibility.
    pub use templar_vault_kernel::types::{ActualIdx, AssetId, ExpectedIdx, TimestampNs};
    pub use templar_vault_kernel::EscrowSettlement;
}

pub mod queue {
    //! Re-export of kernel queue types for backward compatibility.
    pub use templar_vault_kernel::state::queue::{
        can_enqueue, can_partially_satisfy, can_satisfy_withdrawal, compute_full_withdrawal,
        compute_partial_withdrawal, compute_queue_status, compute_settlement,
        compute_settlement_by_price, count_satisfiable, find_request_status, is_past_cooldown,
        is_valid_withdrawal_amount, PendingWithdrawal, QueueStatus, WithdrawalRequestStatus,
        WithdrawalResult, DEFAULT_COOLDOWN_NS, MAX_QUEUE_LENGTH, MIN_WITHDRAWAL_ASSETS,
    };
}
