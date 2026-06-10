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
    UnexpectedEmptyQueue = 5,
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
    AtomicWithdrawRequiresIdle = 34,
    AtomicWithdrawExceedsIdleAssets = 35,
    AtomicWithdrawBurnExceedsTotalShares = 36,
    AtomicWithdrawTotalAssetsUnderflow = 37,
    RebalanceWithdrawRequiresIdle = 38,
    RebalanceWithdrawExceedsExternalAssets = 39,
    RebalanceWithdrawOverflowsIdleAssets = 40,
    WithdrawalLiquidityBelowMinimum = 41,
    RequestWithdrawExpectedAssetsExceedTotalAssets = 42,
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
            Self::UnexpectedEmptyQueue => "withdrawal queue unexpectedly empty",
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
            Self::AtomicWithdrawRequiresIdle => "atomic_withdraw requires Idle",
            Self::AtomicWithdrawExceedsIdleAssets => "atomic_withdraw exceeds idle_assets",
            Self::AtomicWithdrawBurnExceedsTotalShares => {
                "atomic_withdraw burn exceeds total_shares"
            }
            Self::AtomicWithdrawTotalAssetsUnderflow => {
                "atomic_withdraw would underflow total_assets"
            }
            Self::RebalanceWithdrawRequiresIdle => "rebalance_withdraw requires Idle",
            Self::RebalanceWithdrawExceedsExternalAssets => {
                "rebalance_withdraw exceeds external_assets"
            }
            Self::RebalanceWithdrawOverflowsIdleAssets => {
                "rebalance_withdraw would overflow idle_assets"
            }
            Self::WithdrawalLiquidityBelowMinimum => {
                "withdrawal liquidity below minimum payout amount"
            }
            Self::RequestWithdrawExpectedAssetsExceedTotalAssets => {
                "request_withdraw expected assets exceed total_assets"
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl core::fmt::Display for InvalidStateCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.message())
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

#[cfg(not(target_arch = "wasm32"))]
impl core::fmt::Display for InvalidConfigCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.message())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum KernelErrorCode {
    InvalidState = 1000,
    OpIdMismatch = 1001,
    Slippage = 1002,
    MinWithdrawal = 1003,
    QueueFull = 1004,
    NoPendingWithdrawals = 1005,
    Cooldown = 1006,
    Transition = 1007,
    NotImplemented = 1008,
    Restricted = 1009,
    InvalidConfig = 1010,
    ZeroAmount = 1011,
}

impl KernelErrorCode {
    #[inline]
    #[must_use]
    pub const fn index(self) -> u32 {
        self as u32
    }
}

const INVALID_STATE_INDEXED_BASE: u32 = 100_000;
const INVALID_CONFIG_INDEXED_BASE: u32 = 101_000;

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KernelDiagnosticCode {
    Base(KernelErrorCode),
    InvalidState(InvalidStateCode),
    InvalidConfig(InvalidConfigCode),
}

impl KernelDiagnosticCode {
    #[inline]
    #[must_use]
    pub const fn family(self) -> KernelErrorCode {
        match self {
            Self::Base(code) => code,
            Self::InvalidState(_) => KernelErrorCode::InvalidState,
            Self::InvalidConfig(_) => KernelErrorCode::InvalidConfig,
        }
    }

    #[inline]
    #[must_use]
    pub const fn family_code(self) -> u32 {
        self.family().index()
    }

    #[inline]
    #[must_use]
    pub const fn detailed_code(self) -> u32 {
        match self {
            Self::Base(code) => code.index(),
            Self::InvalidState(code) => INVALID_STATE_INDEXED_BASE + code.index() as u32,
            Self::InvalidConfig(code) => INVALID_CONFIG_INDEXED_BASE + code.index() as u32,
        }
    }

    #[inline]
    #[must_use]
    pub const fn index(self) -> u32 {
        self.family_code()
    }

    #[inline]
    #[must_use]
    pub const fn indexed_code(self) -> u32 {
        self.detailed_code()
    }
}

impl From<KernelErrorCode> for KernelDiagnosticCode {
    fn from(code: KernelErrorCode) -> Self {
        Self::Base(code)
    }
}

impl From<InvalidStateCode> for KernelDiagnosticCode {
    fn from(code: InvalidStateCode) -> Self {
        Self::InvalidState(code)
    }
}

impl From<InvalidConfigCode> for KernelDiagnosticCode {
    fn from(code: InvalidConfigCode) -> Self {
        Self::InvalidConfig(code)
    }
}

pub trait HasKernelDiagnosticCode {
    fn diagnostic_code(&self) -> KernelDiagnosticCode;
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
    NoPendingWithdrawals,
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
    pub const fn diagnostic_code(&self) -> KernelDiagnosticCode {
        match self {
            Self::InvalidState(code) => KernelDiagnosticCode::InvalidState(*code),
            Self::OpIdMismatch { .. } => KernelDiagnosticCode::Base(KernelErrorCode::OpIdMismatch),
            Self::Slippage { .. } => KernelDiagnosticCode::Base(KernelErrorCode::Slippage),
            Self::MinWithdrawal { .. } => {
                KernelDiagnosticCode::Base(KernelErrorCode::MinWithdrawal)
            }
            Self::QueueFull { .. } => KernelDiagnosticCode::Base(KernelErrorCode::QueueFull),
            Self::NoPendingWithdrawals => {
                KernelDiagnosticCode::Base(KernelErrorCode::NoPendingWithdrawals)
            }
            Self::Cooldown { .. } => KernelDiagnosticCode::Base(KernelErrorCode::Cooldown),
            Self::Transition(_) => KernelDiagnosticCode::Base(KernelErrorCode::Transition),
            Self::NotImplemented => KernelDiagnosticCode::Base(KernelErrorCode::NotImplemented),
            Self::Restricted(_) => KernelDiagnosticCode::Base(KernelErrorCode::Restricted),
            Self::InvalidConfig(code) => KernelDiagnosticCode::InvalidConfig(*code),
            Self::ZeroAmount => KernelDiagnosticCode::Base(KernelErrorCode::ZeroAmount),
        }
    }

    #[inline]
    #[must_use]
    pub const fn family(&self) -> KernelErrorCode {
        self.diagnostic_code().family()
    }

    #[inline]
    #[must_use]
    pub const fn family_code(&self) -> u32 {
        self.diagnostic_code().family_code()
    }

    #[inline]
    #[must_use]
    pub const fn detailed_code(&self) -> u32 {
        self.diagnostic_code().detailed_code()
    }
}

impl From<&KernelError> for KernelDiagnosticCode {
    fn from(error: &KernelError) -> Self {
        error.diagnostic_code()
    }
}

impl HasKernelDiagnosticCode for KernelDiagnosticCode {
    fn diagnostic_code(&self) -> KernelDiagnosticCode {
        *self
    }
}

impl HasKernelDiagnosticCode for KernelError {
    fn diagnostic_code(&self) -> KernelDiagnosticCode {
        KernelError::diagnostic_code(self)
    }
}

impl HasKernelDiagnosticCode for &KernelError {
    fn diagnostic_code(&self) -> KernelDiagnosticCode {
        KernelError::diagnostic_code(self)
    }
}

impl HasKernelDiagnosticCode for KernelErrorCode {
    fn diagnostic_code(&self) -> KernelDiagnosticCode {
        (*self).into()
    }
}

impl HasKernelDiagnosticCode for InvalidStateCode {
    fn diagnostic_code(&self) -> KernelDiagnosticCode {
        (*self).into()
    }
}

impl HasKernelDiagnosticCode for InvalidConfigCode {
    fn diagnostic_code(&self) -> KernelDiagnosticCode {
        (*self).into()
    }
}

impl From<InvalidStateCode> for KernelError {
    fn from(code: InvalidStateCode) -> Self {
        Self::InvalidState(code)
    }
}

impl From<InvalidConfigCode> for KernelError {
    fn from(code: InvalidConfigCode) -> Self {
        Self::InvalidConfig(code)
    }
}

impl From<TransitionError> for KernelError {
    fn from(error: TransitionError) -> Self {
        Self::Transition(error)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl core::fmt::Display for KernelError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidState(code) => write!(f, "{code} (code {})", self.detailed_code()),
            Self::OpIdMismatch { expected, actual } => {
                write!(f, "op id mismatch: expected {expected}, actual {actual}")
            }
            Self::Slippage { min, actual } => {
                write!(f, "slippage exceeded: min {min}, actual {actual}")
            }
            Self::MinWithdrawal { amount, min } => {
                write!(f, "minimum withdrawal not met: amount {amount}, min {min}")
            }
            Self::QueueFull { current, max } => {
                write!(f, "withdrawal queue full: current {current}, max {max}")
            }
            Self::NoPendingWithdrawals => f.write_str("no pending withdrawals"),
            Self::Cooldown {
                requested_at,
                now,
                cooldown_ns,
            } => write!(
                f,
                "cooldown active: requested_at {requested_at}, now {now}, cooldown_ns {cooldown_ns}"
            ),
            Self::Transition(error) => match error {
                TransitionError::WrongState => f.write_str("transition error: wrong state"),
                TransitionError::OpIdMismatch { expected, actual } => write!(
                    f,
                    "transition error: op id mismatch: expected {expected}, actual {actual}"
                ),
                TransitionError::EmptyAllocationPlan => {
                    f.write_str("transition error: empty allocation plan")
                }
                TransitionError::EmptyRefreshPlan => {
                    f.write_str("transition error: empty refresh plan")
                }
                TransitionError::ZeroWithdrawalAmount => {
                    f.write_str("transition error: zero withdrawal amount")
                }
                TransitionError::ZeroEscrowShares => {
                    f.write_str("transition error: zero escrow shares")
                }
                TransitionError::InvalidIndex { index, max } => {
                    write!(f, "transition error: invalid index: index {index}, max {max}")
                }
                TransitionError::CollectionOverflow {
                    collected,
                    remaining,
                } => write!(
                    f,
                    "transition error: collection overflow: collected {collected}, remaining {remaining}"
                ),
                TransitionError::AllocationOverflow {
                    allocated,
                    remaining,
                } => write!(
                    f,
                    "transition error: allocation overflow: allocated {allocated}, remaining {remaining}"
                ),
                TransitionError::ZeroAllocationAmount => {
                    f.write_str("transition error: zero allocation amount")
                }
                TransitionError::BurnExceedsEscrow { burn, escrow } => write!(
                    f,
                    "transition error: burn exceeds escrow: burn {burn}, escrow {escrow}"
                ),
                TransitionError::WithdrawalIncomplete {
                    remaining,
                    collected,
                } => write!(
                    f,
                    "transition error: withdrawal incomplete: remaining {remaining}, collected {collected}"
                ),
            },
            Self::NotImplemented => f.write_str("action not implemented"),
            Self::Restricted(kind) => match kind {
                RestrictionKind::Paused => f.write_str("restricted: paused"),
                RestrictionKind::Blacklisted => f.write_str("restricted: blacklisted"),
                RestrictionKind::NotWhitelisted => f.write_str("restricted: not whitelisted"),
            },
            Self::InvalidConfig(code) => write!(f, "{code} (code {})", self.detailed_code()),
            Self::ZeroAmount => f.write_str("amount must be greater than zero"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HasKernelDiagnosticCode, InvalidConfigCode, InvalidStateCode, KernelDiagnosticCode,
        KernelError, KernelErrorCode,
    };

    #[test]
    fn kernel_diagnostic_code_from_impls_map_to_expected_variants() {
        assert_eq!(
            KernelDiagnosticCode::from(KernelErrorCode::Slippage),
            KernelDiagnosticCode::Base(KernelErrorCode::Slippage)
        );
        assert_eq!(
            KernelDiagnosticCode::from(InvalidStateCode::DepositRequiresIdle),
            KernelDiagnosticCode::InvalidState(InvalidStateCode::DepositRequiresIdle)
        );
        assert_eq!(
            KernelDiagnosticCode::from(InvalidConfigCode::MaxPendingWithdrawalsExceedsLimit),
            KernelDiagnosticCode::InvalidConfig(
                InvalidConfigCode::MaxPendingWithdrawalsExceedsLimit,
            )
        );
    }

    #[test]
    fn kernel_error_diagnostic_naming_aliases_match_existing_behavior() {
        let error: KernelError = InvalidStateCode::DepositRequiresIdle.into();
        let diagnostic = error.diagnostic_code();

        assert_eq!(diagnostic.family(), KernelErrorCode::InvalidState);
        assert_eq!(diagnostic.family(), error.family());
        assert_eq!(diagnostic.family_code(), error.family_code());
        assert_eq!(diagnostic.detailed_code(), error.detailed_code());
    }

    #[test]
    fn has_kernel_diagnostic_code_trait_is_ergonomic_across_supported_types() {
        let error: KernelError = InvalidConfigCode::MaxPendingWithdrawalsExceedsLimit.into();

        assert_eq!(
            KernelDiagnosticCode::from(&error),
            HasKernelDiagnosticCode::diagnostic_code(&error)
        );
        assert_eq!(
            HasKernelDiagnosticCode::diagnostic_code(&KernelErrorCode::InvalidConfig),
            KernelDiagnosticCode::Base(KernelErrorCode::InvalidConfig)
        );
        assert_eq!(
            HasKernelDiagnosticCode::diagnostic_code(
                &InvalidConfigCode::MaxPendingWithdrawalsExceedsLimit,
            ),
            KernelDiagnosticCode::InvalidConfig(
                InvalidConfigCode::MaxPendingWithdrawalsExceedsLimit,
            )
        );
    }
}
