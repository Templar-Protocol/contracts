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
    Reentrancy = 8,
    AlreadyInitialized = 9,
    MissingConfig = 10,
    ConversionOverflow = 11,
    VaultNotIdle = 12,
    InsufficientIdleAssets = 13,
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
    pub fn transition_error() -> Self {
        Self::KernelError
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

    #[inline]
    pub fn kernel_error(_msg: &str) -> Self {
        Self::KernelError
    }
}

impl From<crate::auth::AuthError> for RuntimeError {
    fn from(err: crate::auth::AuthError) -> Self {
        match err {
            crate::auth::AuthError::NotAuthorized { .. } => RuntimeError::Unauthorized,
            crate::auth::AuthError::InvalidProof => RuntimeError::Unauthorized,
            crate::auth::AuthError::MissingRole => RuntimeError::Unauthorized,
            crate::auth::AuthError::VaultPaused => RuntimeError::InvalidState,
        }
    }
}

#[cfg(test)]
mod tests;
