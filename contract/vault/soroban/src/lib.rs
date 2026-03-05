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
pub mod policy;
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
    market::{
        AttemptId, MarketRef, SettlementReceipt, SorobanCrossChainMarketAdapter,
        SorobanMarketAdapter, TestCrossChainAdapter, TestMarketAdapter,
    },
    policy::{
        build_allocation_plan_with_locks, build_refresh_plan_with_locks,
        build_withdrawal_plan_with_locks, filter_allocation_plan, filter_unlocked_targets,
        MarketLock, MarketLockSet,
    },
    rbac::{RbacAuth, RbacConfig, Role, RoleAssignment},
    soroban_sdk::{Address, Bytes, Env},
    storage::{
        MemoryStorage, SorobanStorage, SorobanStorageKey, Storage, StorageVersion, VersionedState,
    },
};
