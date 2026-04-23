//! Error types for the Soroban ERC-4626 proxy crate.

use soroban_sdk::contracterror;

use templar_soroban_shared_types::CodecError;

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    InvalidInput = 3,
    VaultError = 4,
    InsufficientAllowance = 5,
    NotImplemented = 6,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum RuntimeError {
    NotInitialized,
    AlreadyInitialized,
    VaultError,
    InsufficientAllowance,
    NotImplemented,
    InvalidInput,
}

impl From<RuntimeError> for ContractError {
    fn from(error: RuntimeError) -> Self {
        match error {
            RuntimeError::NotInitialized => Self::NotInitialized,
            RuntimeError::AlreadyInitialized => Self::AlreadyInitialized,
            RuntimeError::VaultError => Self::VaultError,
            RuntimeError::InsufficientAllowance => Self::InsufficientAllowance,
            RuntimeError::NotImplemented => Self::NotImplemented,
            RuntimeError::InvalidInput => Self::InvalidInput,
        }
    }
}

impl From<CodecError> for ContractError {
    fn from(_: CodecError) -> Self {
        Self::InvalidInput
    }
}

impl From<CodecError> for RuntimeError {
    fn from(_: CodecError) -> Self {
        Self::InvalidInput
    }
}

impl ContractError {
    pub(crate) const fn from_vault_error_code(code: u32) -> Self {
        match code {
            3 => Self::InvalidInput,
            8 => Self::AlreadyInitialized,
            _ => Self::VaultError,
        }
    }
}
