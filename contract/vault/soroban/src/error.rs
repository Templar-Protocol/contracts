//! Runtime error types.

use alloc::string::String;
use soroban_sdk::contracterror;

/// Contract-facing error codes for Soroban entrypoints.
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
    Reentrancy = 8,
    AlreadyInitialized = 9,
    MissingConfig = 10,
    ConversionOverflow = 11,
    /// Vault is not in Idle state (required for atomic withdraw/redeem).
    VaultNotIdle = 12,
    /// Insufficient idle assets for the requested withdrawal.
    InsufficientIdleAssets = 13,

    // OpenZeppelin Pausable errors (1000-1099)
    /// The operation failed because the contract is already paused.
    EnforcedPause = 1000,
    /// The operation failed because the contract is not paused.
    ExpectedPause = 1001,

    // OpenZeppelin Upgradeable errors (1100-1199)
    /// Migration attempted without a preceding upgrade.
    MigrationNotAllowed = 1100,
}

/// Errors that can occur during runtime execution.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum RuntimeError {
    /// Authorization failed.
    Unauthorized(String),
    /// Insufficient balance for the operation.
    InsufficientBalance { available: u128, required: u128 },
    /// Invalid operation state.
    InvalidState(String),
    /// Storage error.
    StorageError(String),
    /// Effect execution failed.
    EffectFailed(String),
    /// Invalid input parameter.
    InvalidInput(String),
    /// Kernel transition error.
    KernelError(String),
}

impl From<RuntimeError> for ContractError {
    fn from(err: RuntimeError) -> Self {
        match err {
            RuntimeError::Unauthorized(_) => ContractError::Unauthorized,
            RuntimeError::InsufficientBalance { .. } => ContractError::InsufficientBalance,
            RuntimeError::InvalidState(_) => ContractError::InvalidState,
            RuntimeError::StorageError(_) => ContractError::StorageError,
            RuntimeError::EffectFailed(_) => ContractError::EffectFailed,
            RuntimeError::InvalidInput(_) => ContractError::InvalidInput,
            RuntimeError::KernelError(_) => ContractError::KernelError,
        }
    }
}

impl RuntimeError {
    /// Create an unauthorized error with a message.
    #[inline]
    #[must_use]
    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self::Unauthorized(msg.into())
    }

    /// Create a contract error (alias for invalid_state).
    #[inline]
    #[must_use]
    pub fn contract_error(msg: impl Into<String>) -> Self {
        Self::InvalidState(msg.into())
    }

    /// Create a transition error (alias for kernel_error).
    #[inline]
    #[must_use]
    pub fn transition_error() -> Self {
        Self::KernelError(String::from("transition failed"))
    }

    /// Create an insufficient balance error.
    #[inline]
    #[must_use]
    pub const fn insufficient_balance(available: u128, required: u128) -> Self {
        Self::InsufficientBalance {
            available,
            required,
        }
    }

    /// Create an invalid state error.
    #[inline]
    #[must_use]
    pub fn invalid_state(msg: impl Into<String>) -> Self {
        Self::InvalidState(msg.into())
    }

    /// Create a storage error.
    #[inline]
    #[must_use]
    pub fn storage_error(msg: impl Into<String>) -> Self {
        Self::StorageError(msg.into())
    }

    /// Create an effect failed error.
    #[inline]
    #[must_use]
    pub fn effect_failed(msg: impl Into<String>) -> Self {
        Self::EffectFailed(msg.into())
    }

    /// Create an invalid input error.
    #[inline]
    #[must_use]
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }

    /// Create a kernel error.
    #[inline]
    #[must_use]
    pub fn kernel_error(msg: impl Into<String>) -> Self {
        Self::KernelError(msg.into())
    }
}

impl core::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unauthorized(msg)
            | Self::InvalidState(msg)
            | Self::StorageError(msg)
            | Self::EffectFailed(msg)
            | Self::InvalidInput(msg)
            | Self::KernelError(msg) => write!(f, "{msg}"),
            Self::InsufficientBalance {
                available,
                required,
            } => {
                write!(
                    f,
                    "insufficient balance: available={available}, required={required}"
                )
            }
        }
    }
}

impl From<crate::auth::AuthError> for RuntimeError {
    fn from(err: crate::auth::AuthError) -> Self {
        match err {
            crate::auth::AuthError::NotAuthorized { .. } => {
                RuntimeError::unauthorized("not authorized")
            }
            crate::auth::AuthError::InvalidProof => RuntimeError::unauthorized("invalid proof"),
            crate::auth::AuthError::MissingRole(_) => {
                RuntimeError::unauthorized("missing required role")
            }
            crate::auth::AuthError::VaultPaused => RuntimeError::invalid_state("vault is paused"),
        }
    }
}

#[cfg(test)]
mod tests;
