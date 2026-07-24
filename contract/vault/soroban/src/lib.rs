//! Soroban effect interpreter and runtime for Templar Protocol vaults.
//!
//! This crate provides the chain-specific runtime for executing vault kernel
//! effects on Soroban. It includes:
//!
//! - Effect interpreter for processing kernel effects
//! - Auth adapter interface for pluggable authorization (RBAC, Merkle)
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
//! The default runtime forwards the five production kernel action gates:
//! recovery, external synchronization, fee refresh, allocation lifecycle, and
//! refresh lifecycle. `action-pause` is available as an opt-in build feature.
//! The callable `version()` entrypoint reports the package version and exact
//! compiled runtime-capability mask. The reserved companion-upgrade capability
//! remains unset until the runtime can authorize companion-contract upgrades.
//!
//! - `std` - Enable std library support (for testing)

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

use templar_soroban_shared_types::{
    RUNTIME_FEATURE_ACTION_ALLOCATION_LIFECYCLE, RUNTIME_FEATURE_ACTION_PAUSE,
    RUNTIME_FEATURE_ACTION_RECOVERY, RUNTIME_FEATURE_ACTION_REFRESH_FEES,
    RUNTIME_FEATURE_ACTION_REFRESH_LIFECYCLE, RUNTIME_FEATURE_ACTION_SYNC_EXTERNAL,
};

/// Package version compiled into this runtime artifact.
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

const fn feature_flag(enabled: bool, flag: u64) -> u64 {
    if enabled {
        flag
    } else {
        0
    }
}

/// Runtime capabilities compiled into this artifact.
pub const RUNTIME_FEATURE_FLAGS: u64 = feature_flag(
    templar_vault_kernel::ACTION_RECOVERY_ENABLED,
    RUNTIME_FEATURE_ACTION_RECOVERY,
) | feature_flag(
    templar_vault_kernel::ACTION_SYNC_EXTERNAL_ENABLED,
    RUNTIME_FEATURE_ACTION_SYNC_EXTERNAL,
) | feature_flag(
    templar_vault_kernel::ACTION_REFRESH_FEES_ENABLED,
    RUNTIME_FEATURE_ACTION_REFRESH_FEES,
) | feature_flag(
    templar_vault_kernel::ACTION_ALLOCATION_LIFECYCLE_ENABLED,
    RUNTIME_FEATURE_ACTION_ALLOCATION_LIFECYCLE,
) | feature_flag(
    templar_vault_kernel::ACTION_REFRESH_LIFECYCLE_ENABLED,
    RUNTIME_FEATURE_ACTION_REFRESH_LIFECYCLE,
) | feature_flag(
    templar_vault_kernel::ACTION_PAUSE_ENABLED,
    RUNTIME_FEATURE_ACTION_PAUSE,
);

pub mod auth;
pub mod contract;
pub(crate) mod convert;
pub mod effects;
pub mod error;
pub mod fungible_vault;
pub mod market;
pub mod storage;

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
    market::{invoke_progress_withdrawal, invoke_supply, invoke_total_assets, SorobanMarketMethod},
    rbac::{RbacAuth, RbacConfig, Role, RoleAssignment},
    soroban_sdk::{Address, Bytes, Env},
    storage::{SorobanStorage, SorobanStorageKey, Storage},
    templar_curator_primitives::policy::market_lock::{MarketLease, MarketLeaseRegistry},
};

#[cfg(any(test, feature = "testutils"))]
pub mod test_utils;

#[cfg(test)]
mod tests;
