//! Soroban effect interpreter and runtime for Templar Protocol vaults.
//!
//! This crate provides the chain-specific runtime for executing vault kernel
//! effects on Soroban. It includes:
//!
//! - Effect interpreter for processing kernel effects
//! - Auth adapter interface for pluggable authorization (RBAC, Merkle)
//! - Storage versioning wrapper for state persistence
//! - SEP-41 token integration helpers
//! - Curator vault contract with entrypoints
//!
//! # Architecture
//!
//! The Soroban runtime acts as the "executor" layer that:
//! 1. Receives user actions (deposit, withdraw, etc.)
//! 2. Validates authorization via [`AuthAdapter`]
//! 3. Dispatches to kernel transitions
//! 4. Interprets returned [`KernelEffect`]s via [`EffectInterpreter`]
//! 5. Persists state via [`Storage`]
//!
//! # Feature Flags
//!
//! - `std` - Enable std library support (for testing)

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod auth;
pub mod contract;
pub(crate) mod convert;
pub mod effects;
pub mod error;
pub mod fungible_vault;
pub mod market;
pub mod storage;

#[cfg(any(test, feature = "testutils"))]
pub use storage::MemoryStorage;

pub mod rbac {
    pub use templar_curator_primitives::rbac::{RbacAuth, RbacConfig, Role, RoleAssignment};
}
pub use {
    auth::{ActionKind, AuthAdapter, AuthError, SorobanAuth},
    contract::{
        AllocationResult, ContractConfig, CuratorVault, DepositResult, RefreshResult,
        SorobanVaultContract, VaultDataKey, WithdrawRequestResult,
    },
    effects::{
        AddressMap, AddressRegistrar, EffectContext, EffectInterpreter, EffectResult,
        EffectSummary, SdkTokenAdapter, Sep41Token, SorobanEffectInterpreter,
    },
    error::{ContractError, RuntimeError},
    market::{invoke_progress_withdrawal, invoke_supply, invoke_total_assets},
    rbac::{RbacAuth, RbacConfig, Role, RoleAssignment},
    soroban_sdk::{Address, Bytes, Env},
    storage::{SorobanStorage, SorobanStorageKey, Storage, StorageVersion, VersionedState},
    templar_curator_primitives::policy::market_lock::{MarketLock, MarketLockSet},
};

#[cfg(test)]
mod tests;
