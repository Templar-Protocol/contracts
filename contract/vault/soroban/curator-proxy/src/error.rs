//! Error types for the Soroban curator operations proxy.

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
    GovernanceError = 5,
    NotImplemented = 6,
    SelectorAmbiguous = 7,
}

impl From<CodecError> for ContractError {
    fn from(_: CodecError) -> Self {
        Self::InvalidInput
    }
}
