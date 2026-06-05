//! Error types for the Soroban curator operations proxy.

use soroban_sdk::contracterror;

use templar_soroban_shared_types::{
    CodecError, VAULT_ERR_ALREADY_INITIALIZED, VAULT_ERR_INVALID_INPUT,
};

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    InvalidInput = 3,
    VaultError = 4,
    GovernanceError = 5,
    NotImplemented = 6,
}

impl From<CodecError> for ContractError {
    fn from(_: CodecError) -> Self {
        Self::InvalidInput
    }
}

impl ContractError {
    pub(crate) const fn from_vault_error_code(code: u32) -> Self {
        match code {
            VAULT_ERR_INVALID_INPUT => Self::InvalidInput,
            VAULT_ERR_ALREADY_INITIALIZED => Self::AlreadyInitialized,
            _ => Self::VaultError,
        }
    }
}
