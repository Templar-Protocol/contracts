//! Kernel error types.

use crate::restrictions::RestrictionKind;
use crate::transitions::TransitionError;

/// Indexed invalid-state reasons for stable wasm diagnostics.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum InvalidStateCode {
    Unknown = 0,
    WithdrawalQueueHeadMismatch = 1,
    FeeMintOverflowTotalSupply = 2,
    WithdrawalQueueCacheOverflow = 3,
    WithdrawalQueueMissingEntry = 4,
    WithdrawalQueueEmpty = 5,
    WithdrawalQueueInvariantViolation = 6,
    DepositRequiresIdle = 7,
    DepositOverflowTotalAssets = 8,
    DepositOverflowIdleAssets = 9,
    MintOverflowTotalShares = 10,
    RequestWithdrawRequiresIdle = 11,
    ExecuteWithdrawRequiresIdle = 12,
    ExecuteWithdrawRequiresIdleUseCallbacks = 13,
    StartAllocationMustReturnAllocating = 14,
    AllocationPlanExceedsIdleAssets = 15,
    SyncExternalRequiresActiveOp = 16,
    SyncExternalRequiresAllowedStates = 17,
    SyncExternalOverflowIdlePlusExternal = 18,
    SyncExternalWouldMoreThanDoubleTotalAssets = 19,
    AbortRefreshingRequiresActiveOp = 20,
    AbortRefreshingRequiresRefreshing = 21,
    AbortAllocatingRequiresAllocating = 22,
    AbortAllocatingRestoreIdleMismatch = 23,
    AbortWithdrawingRequiresWithdrawing = 24,
    AbortWithdrawingRefundMismatch = 25,
    SettlePayoutRequiresPayout = 26,
    PayoutSuccessSettlementMismatch = 27,
    PayoutBurnExceedsTotalShares = 28,
    PayoutFailureSettlementMismatch = 29,
    PayoutFailureRestoreIdleMismatch = 30,
    RefreshFeesRequiresIdle = 31,
    FeeRefreshTimestampMustAdvance = 32,
    EmergencyResetAlreadyIdle = 33,
}

impl InvalidStateCode {
    #[inline]
    #[must_use]
    pub const fn index(self) -> u16 {
        self as u16
    }

    #[inline]
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Unknown => "invalid state",
            Self::WithdrawalQueueHeadMismatch => "withdrawal queue head mismatch",
            Self::FeeMintOverflowTotalSupply => "fee minting would overflow total_supply",
            Self::WithdrawalQueueCacheOverflow => "withdrawal queue cache overflow",
            Self::WithdrawalQueueMissingEntry => "withdrawal queue missing entry",
            Self::WithdrawalQueueEmpty => "withdrawal queue empty",
            Self::WithdrawalQueueInvariantViolation => "withdrawal queue invariant violation",
            Self::DepositRequiresIdle => "deposit requires Idle",
            Self::DepositOverflowTotalAssets => "deposit would overflow total_assets",
            Self::DepositOverflowIdleAssets => "deposit would overflow idle_assets",
            Self::MintOverflowTotalShares => "minting would overflow total_shares",
            Self::RequestWithdrawRequiresIdle => "request_withdraw requires Idle",
            Self::ExecuteWithdrawRequiresIdle => "execute_withdraw requires Idle",
            Self::ExecuteWithdrawRequiresIdleUseCallbacks => {
                "execute_withdraw requires Idle (use withdrawal callbacks to advance)"
            }
            Self::StartAllocationMustReturnAllocating => "start_allocation must return Allocating",
            Self::AllocationPlanExceedsIdleAssets => "allocation plan exceeds idle_assets",
            Self::SyncExternalRequiresActiveOp => "sync_external_assets requires active op",
            Self::SyncExternalRequiresAllowedStates => {
                "sync_external_assets requires Allocating/Withdrawing/Refreshing"
            }
            Self::SyncExternalOverflowIdlePlusExternal => {
                "sync_external_assets overflow: idle + external exceeds u128"
            }
            Self::SyncExternalWouldMoreThanDoubleTotalAssets => {
                "sync_external_assets would more than double total_assets"
            }
            Self::AbortRefreshingRequiresActiveOp => "abort_refreshing requires active op",
            Self::AbortRefreshingRequiresRefreshing => "abort_refreshing requires Refreshing",
            Self::AbortAllocatingRequiresAllocating => "abort_allocating requires Allocating",
            Self::AbortAllocatingRestoreIdleMismatch => "abort_allocating restore_idle mismatch",
            Self::AbortWithdrawingRequiresWithdrawing => "abort_withdrawing requires Withdrawing",
            Self::AbortWithdrawingRefundMismatch => "abort_withdrawing refund_shares mismatch",
            Self::SettlePayoutRequiresPayout => "settle_payout requires Payout",
            Self::PayoutSuccessSettlementMismatch => "payout success settlement mismatch",
            Self::PayoutBurnExceedsTotalShares => "payout burn exceeds total_shares",
            Self::PayoutFailureSettlementMismatch => "payout failure settlement mismatch",
            Self::PayoutFailureRestoreIdleMismatch => {
                "payout failure restore_idle must equal payout.amount"
            }
            Self::RefreshFeesRequiresIdle => "refresh_fees requires Idle",
            Self::FeeRefreshTimestampMustAdvance => "fee refresh timestamp must advance",
            Self::EmergencyResetAlreadyIdle => "emergency_reset: vault is already Idle",
        }
    }
}

/// Indexed invalid-config reasons for stable wasm diagnostics.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum InvalidConfigCode {
    Unknown = 0,
    MaxPendingWithdrawalsExceedsLimit = 1,
}

impl InvalidConfigCode {
    #[inline]
    #[must_use]
    pub const fn index(self) -> u16 {
        self as u16
    }

    #[inline]
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Unknown => "invalid config",
            Self::MaxPendingWithdrawalsExceedsLimit => {
                "max_pending_withdrawals exceeds MAX_PENDING"
            }
        }
    }
}

/// Errors that can occur when applying kernel actions.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum KernelError {
    InvalidState(InvalidStateCode),
    OpIdMismatch {
        expected: u64,
        actual: u64,
    },
    Slippage {
        min: u128,
        actual: u128,
    },
    MinWithdrawal {
        amount: u128,
        min: u128,
    },
    QueueFull {
        current: u32,
        max: u32,
    },
    EmptyQueue,
    Cooldown {
        requested_at: u64,
        now: u64,
        cooldown_ns: u64,
    },
    Transition(TransitionError),
    NotImplemented,
    Restricted(RestrictionKind),
    InvalidConfig(InvalidConfigCode),
    ZeroAmount,
}

impl KernelError {
    #[inline]
    #[must_use]
    pub const fn invalid_state_code(code: InvalidStateCode) -> Self {
        Self::InvalidState(code)
    }

    #[inline]
    #[must_use]
    pub const fn invalid_state(message: &'static str) -> Self {
        let _ = message;
        Self::InvalidState(InvalidStateCode::Unknown)
    }

    #[inline]
    #[must_use]
    pub const fn invalid_config_code(code: InvalidConfigCode) -> Self {
        Self::InvalidConfig(code)
    }

    #[inline]
    #[must_use]
    pub const fn invalid_config(message: &'static str) -> Self {
        let _ = message;
        Self::InvalidConfig(InvalidConfigCode::Unknown)
    }

    /// Stable numeric code for on-chain debugging and indexing.
    #[must_use]
    pub fn code(&self) -> u32 {
        match self {
            KernelError::InvalidState(_) => 1000,
            KernelError::OpIdMismatch { .. } => 1001,
            KernelError::Slippage { .. } => 1002,
            KernelError::MinWithdrawal { .. } => 1003,
            KernelError::QueueFull { .. } => 1004,
            KernelError::EmptyQueue => 1005,
            KernelError::Cooldown { .. } => 1006,
            KernelError::Transition(_) => 1007,
            KernelError::NotImplemented => 1008,
            KernelError::Restricted(_) => 1009,
            KernelError::InvalidConfig(_) => 1010,
            KernelError::ZeroAmount => 1011,
        }
    }

    /// Stable indexed code with finer-grained invalid-state/config reason.
    #[must_use]
    pub fn indexed_code(&self) -> u32 {
        match self {
            KernelError::InvalidState(code) => 100_000 + u32::from(code.index()),
            KernelError::InvalidConfig(code) => 101_000 + u32::from(code.index()),
            _ => self.code(),
        }
    }
}
