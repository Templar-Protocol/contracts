//! Runtime error types.

use soroban_sdk::contracterror;

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    Unauthorized = 1,
    InvalidState = 2,
    InvalidInput = 3,
    InsufficientBalance = 4,
    StorageError = 5,
    EffectFailed = 6,
    KernelError = 7,
    AlreadyInitialized = 8,
    MissingConfig = 9,
    ConversionOverflow = 10,
    VaultNotIdle = 11,
    InsufficientIdleAssets = 12,
    Throttled = 13,
    EpochCutoffRejected = 14,
    EpochSettlementRejected = 15,
    EpochIntakeNotOpen = 16,
    EpochDrainRequired = 17,
    WithdrawalEpochUnsettled = 18,
    CancelRequestNotFound = 19,
    ReportMetadataUnavailable = 20,
    EpochSnapshotUnavailable = 21,
    EnforcedPause = 1000,
    ExpectedPause = 1001,
    MigrationNotAllowed = 1100,
}

/// Errors that can occur during runtime execution.
/// Error messages stripped in WASM to reduce binary size.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    Unauthorized,
    InsufficientBalance,
    InvalidState,
    StorageError,
    EffectFailed,
    InvalidInput,
    KernelError,
    VaultNotIdle,
    EpochCutoffRejected,
    EpochSettlementRejected,
    EpochIntakeNotOpen,
    EpochDrainRequired,
    WithdrawalEpochUnsettled,
    CancelRequestNotFound,
    ReportMetadataUnavailable,
    EpochSnapshotUnavailable,
    InsufficientIdleAssets,
    Throttled,
    ConversionOverflow,
    MissingConfig,
    EnforcedPause,
}

impl From<RuntimeError> for ContractError {
    fn from(err: RuntimeError) -> Self {
        match err {
            RuntimeError::Unauthorized => ContractError::Unauthorized,
            RuntimeError::InsufficientBalance => ContractError::InsufficientBalance,
            RuntimeError::InvalidState => ContractError::InvalidState,
            RuntimeError::StorageError => ContractError::StorageError,
            RuntimeError::EffectFailed => ContractError::EffectFailed,
            RuntimeError::InvalidInput => ContractError::InvalidInput,
            RuntimeError::KernelError => ContractError::KernelError,
            RuntimeError::VaultNotIdle => ContractError::VaultNotIdle,
            RuntimeError::EpochCutoffRejected => ContractError::EpochCutoffRejected,
            RuntimeError::EpochSettlementRejected => ContractError::EpochSettlementRejected,
            RuntimeError::EpochIntakeNotOpen => ContractError::EpochIntakeNotOpen,
            RuntimeError::EpochDrainRequired => ContractError::EpochDrainRequired,
            RuntimeError::WithdrawalEpochUnsettled => ContractError::WithdrawalEpochUnsettled,
            RuntimeError::CancelRequestNotFound => ContractError::CancelRequestNotFound,
            RuntimeError::ReportMetadataUnavailable => ContractError::ReportMetadataUnavailable,
            RuntimeError::EpochSnapshotUnavailable => ContractError::EpochSnapshotUnavailable,
            RuntimeError::InsufficientIdleAssets => ContractError::InsufficientIdleAssets,
            RuntimeError::Throttled => ContractError::Throttled,
            RuntimeError::ConversionOverflow => ContractError::ConversionOverflow,
            RuntimeError::MissingConfig => ContractError::MissingConfig,
            RuntimeError::EnforcedPause => ContractError::EnforcedPause,
        }
    }
}

impl RuntimeError {
    #[inline]
    pub fn unauthorized(_msg: &str) -> Self {
        Self::Unauthorized
    }

    #[inline]
    pub fn contract_error(_msg: &str) -> Self {
        Self::InvalidState
    }

    #[inline]
    pub const fn insufficient_balance(_available: u128, _required: u128) -> Self {
        Self::InsufficientBalance
    }

    #[inline]
    pub fn invalid_state(_msg: &str) -> Self {
        Self::InvalidState
    }

    #[inline]
    pub fn storage_error(_msg: &str) -> Self {
        Self::StorageError
    }

    #[inline]
    pub fn effect_failed(_msg: &str) -> Self {
        Self::EffectFailed
    }

    #[inline]
    pub fn invalid_input(_msg: &str) -> Self {
        Self::InvalidInput
    }
}

impl From<crate::auth::AuthError> for RuntimeError {
    fn from(err: crate::auth::AuthError) -> Self {
        match err {
            crate::auth::AuthError::NotAuthorized { .. } => RuntimeError::Unauthorized,
            crate::auth::AuthError::InvalidProof => RuntimeError::Unauthorized,
            crate::auth::AuthError::MissingRole { .. } => RuntimeError::Unauthorized,
            crate::auth::AuthError::VaultPaused => RuntimeError::InvalidState,
        }
    }
}

impl From<templar_curator_primitives::policy::state::PolicyStateError> for RuntimeError {
    fn from(_err: templar_curator_primitives::policy::state::PolicyStateError) -> Self {
        RuntimeError::InvalidState
    }
}

/// Map a kernel rejection to the runtime code that names the law it broke.
///
/// Every kernel error variant is classified deliberately, so no rejection is
/// reported as the generic `KernelError` and unrelated rejections can never
/// satisfy one another's assertions. Adding a variant to `KernelError`,
/// `InvalidStateCode`, `TransitionError`, `InvalidConfigCode`, or
/// `RestrictionKind` makes this match fail to compile until the new law is
/// classified here on purpose. `RuntimeError::KernelError` is deliberately
/// absent from this table.
impl From<templar_vault_kernel::error::KernelError> for RuntimeError {
    fn from(err: templar_vault_kernel::error::KernelError) -> Self {
        use templar_vault_kernel::error::{InvalidConfigCode, InvalidStateCode, KernelError};
        use templar_vault_kernel::RestrictionKind;

        match err {
            KernelError::InvalidState(code) => match code {
                InvalidStateCode::CancelCallerNotOwner
                | InvalidStateCode::EpochCutoffUnauthorized => RuntimeError::Unauthorized,
                InvalidStateCode::CancelRequestNotFound => RuntimeError::CancelRequestNotFound,
                InvalidStateCode::DepositRequiresIdle
                | InvalidStateCode::RequestWithdrawRequiresIdle
                | InvalidStateCode::ExecuteWithdrawRequiresIdle
                | InvalidStateCode::ExecuteWithdrawRequiresIdleUseCallbacks
                | InvalidStateCode::AtomicWithdrawRequiresIdle
                | InvalidStateCode::RebalanceWithdrawRequiresIdle
                | InvalidStateCode::RefreshFeesRequiresIdle
                | InvalidStateCode::EpochRequiresIdle
                | InvalidStateCode::EpochCutoffRequiresIdle => RuntimeError::VaultNotIdle,
                InvalidStateCode::WithdrawalLiquidityBelowMinimum
                | InvalidStateCode::AtomicWithdrawExceedsIdleAssets => {
                    RuntimeError::InsufficientIdleAssets
                }
                InvalidStateCode::WithdrawalEpochUnsettled => RuntimeError::WithdrawalEpochUnsettled,
                InvalidStateCode::WithdrawalBelowMinAssetsOut => RuntimeError::InvalidInput,
                InvalidStateCode::EpochCutoffRejected => RuntimeError::EpochCutoffRejected,
                InvalidStateCode::EpochSettlementRejected
                | InvalidStateCode::EpochSettledNavImmutable => RuntimeError::EpochSettlementRejected,
                InvalidStateCode::EpochIntakeNotOpen
                | InvalidStateCode::DepositEpochUnsettled => RuntimeError::EpochIntakeNotOpen,
                InvalidStateCode::EpochDrainRequired => RuntimeError::EpochDrainRequired,
                #[cfg(feature = "epoch")]
                InvalidStateCode::EpochSeedRejected => RuntimeError::InvalidState,
                InvalidStateCode::Unknown
                | InvalidStateCode::FeeMintOverflowTotalSupply
                | InvalidStateCode::DepositOverflowTotalAssets
                | InvalidStateCode::DepositOverflowIdleAssets
                | InvalidStateCode::MintOverflowTotalShares
                | InvalidStateCode::SyncExternalOverflowIdlePlusExternal
                | InvalidStateCode::AtomicWithdrawTotalAssetsUnderflow
                | InvalidStateCode::RebalanceWithdrawOverflowsIdleAssets
                | InvalidStateCode::DepositAdmissionOverflowTotalAssets
                | InvalidStateCode::DepositAdmissionOverflowIdleAssets => {
                    RuntimeError::ConversionOverflow
                }
                InvalidStateCode::WithdrawalQueueHeadMismatch
                | InvalidStateCode::WithdrawalQueueCacheOverflow
                | InvalidStateCode::WithdrawalQueueMissingEntry
                | InvalidStateCode::UnexpectedEmptyQueue
                | InvalidStateCode::WithdrawalQueueInvariantViolation
                | InvalidStateCode::StartAllocationMustReturnAllocating
                | InvalidStateCode::AllocationPlanExceedsIdleAssets
                | InvalidStateCode::SyncExternalRequiresActiveOp
                | InvalidStateCode::SyncExternalRequiresAllowedStates
                | InvalidStateCode::AbortRefreshingRequiresActiveOp
                | InvalidStateCode::AbortRefreshingRequiresRefreshing
                | InvalidStateCode::AbortAllocatingRequiresAllocating
                | InvalidStateCode::AbortAllocatingRestoreIdleMismatch
                | InvalidStateCode::AbortWithdrawingRequiresWithdrawing
                | InvalidStateCode::AbortWithdrawingRefundMismatch
                | InvalidStateCode::SettlePayoutRequiresPayout
                | InvalidStateCode::PayoutSuccessSettlementMismatch
                | InvalidStateCode::PayoutBurnExceedsTotalShares
                | InvalidStateCode::PayoutFailureSettlementMismatch
                | InvalidStateCode::PayoutFailureRestoreIdleMismatch
                | InvalidStateCode::FeeRefreshTimestampMustAdvance
                | InvalidStateCode::EmergencyResetAlreadyIdle
                | InvalidStateCode::AtomicWithdrawBurnExceedsTotalShares
                | InvalidStateCode::RebalanceWithdrawExceedsExternalAssets
                | InvalidStateCode::CancelInFlightRequest
                | InvalidStateCode::CancelQueueRepairFailed
                | InvalidStateCode::PayoutBurnMustMatchFullEscrow
                | InvalidStateCode::PayoutClaimMismatch => RuntimeError::InvalidState,
            },
            KernelError::Slippage { .. }
            | KernelError::MinWithdrawal { .. }
            | KernelError::ZeroAmount => RuntimeError::InvalidInput,
            KernelError::Restricted(RestrictionKind::Paused) => RuntimeError::EnforcedPause,
            KernelError::Restricted(
                RestrictionKind::Blacklisted | RestrictionKind::NotWhitelisted,
            ) => RuntimeError::Unauthorized,
            KernelError::Cooldown { .. } => RuntimeError::Throttled,
            KernelError::InvalidConfig(
                InvalidConfigCode::Unknown
                | InvalidConfigCode::MaxPendingWithdrawalsExceedsLimit,
            )
            | KernelError::NotImplemented => RuntimeError::MissingConfig,
            KernelError::OpIdMismatch { .. }
            | KernelError::QueueFull { .. }
            | KernelError::NoPendingWithdrawals => RuntimeError::InvalidState,
            KernelError::Transition(err) => RuntimeError::from(err),
        }
    }
}

/// Map a state-transition rejection to the runtime code that names the law it
/// broke. Every `TransitionError` variant is classified, so `KernelError`,
/// `WithdrawalEpochUnsettled`, `InsufficientIdleAssets`, and `EnforcedPause`
/// stay reserved for the rejections that name them, and a new variant makes
/// this build fail until it is classified on purpose.
impl From<templar_vault_kernel::TransitionError> for RuntimeError {
    fn from(err: templar_vault_kernel::TransitionError) -> Self {
        use templar_vault_kernel::TransitionError;

        match err {
            TransitionError::ZeroWithdrawalAmount
            | TransitionError::ZeroEscrowShares
            | TransitionError::ZeroAllocationAmount
            | TransitionError::EmptyAllocationPlan
            | TransitionError::EmptyRefreshPlan
            | TransitionError::InvalidIndex { .. }
            | TransitionError::WithdrawalIncomplete { .. } => RuntimeError::InvalidInput,
            TransitionError::WrongState
            | TransitionError::OpIdMismatch { .. }
            | TransitionError::CollectionOverflow { .. }
            | TransitionError::AllocationOverflow { .. }
            | TransitionError::BurnExceedsEscrow { .. } => RuntimeError::InvalidState,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContractError, RuntimeError};
    use templar_vault_kernel::error::{InvalidStateCode, KernelError};
    use templar_vault_kernel::RestrictionKind;

    #[test]
    fn eng_697_law_rejections_collapse_to_distinct_codes() {
        let codes = [
            ContractError::from(RuntimeError::from(KernelError::from(
                InvalidStateCode::CancelCallerNotOwner,
            ))),
            ContractError::from(RuntimeError::from(KernelError::from(
                InvalidStateCode::WithdrawalEpochUnsettled,
            ))),
            ContractError::from(RuntimeError::from(KernelError::Slippage {
                min: 1_000,
                actual: 999,
            })),
            ContractError::from(RuntimeError::from(KernelError::from(
                InvalidStateCode::WithdrawalLiquidityBelowMinimum,
            ))),
            ContractError::from(RuntimeError::from(KernelError::Restricted(
                RestrictionKind::Paused,
            ))),
        ];
        assert_eq!(
            codes,
            [
                ContractError::Unauthorized,
                ContractError::WithdrawalEpochUnsettled,
                ContractError::InvalidInput,
                ContractError::InsufficientIdleAssets,
                ContractError::EnforcedPause,
            ]
        );
        assert_eq!(
            ContractError::from(RuntimeError::from(KernelError::Restricted(
                RestrictionKind::Blacklisted
            ))),
            ContractError::Unauthorized
        );
    }
}
