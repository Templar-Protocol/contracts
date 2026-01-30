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
pub mod effects;
pub mod error;
pub mod market;
pub mod policy;
pub mod rbac;
pub mod reconciliation;
pub mod storage;

// Re-exports for convenience
pub use auth::{ActionKind, AuthAdapter, AuthError};
pub use contract::{
    AllocationResult, ContractConfig, CuratorVault, DepositResult, RefreshResult,
    WithdrawRequestResult,
};
pub use effects::{EffectContext, EffectInterpreter, EffectResult, EffectSummary, MockInterpreter};
pub use error::RuntimeError;
pub use market::{
    AttemptId, Bytes, CrossChainMarketAdapter, Env, MarketAdapter, MarketRef,
    MockSorobanCrossChainAdapter, MockSorobanMarketAdapter, SettlementReceipt, SorobanAddress,
    SorobanCrossChainMarketAdapter, SorobanMarketAdapter,
};
pub use rbac::{RbacAuth, RbacConfig, Role, RoleAssignment};
pub use reconciliation::{
    build_refresh_plan, reconcile_external_assets, resync_external_assets, ReconciliationEvent,
    ReconciliationRecord, ResyncRequest, ResyncResult,
};
pub use storage::{MemoryStorage, Storage, StorageVersion, VersionedState};

// Policy re-exports for convenience
pub use policy::{
    build_allocation_plan_with_locks, build_refresh_plan_with_locks,
    build_withdrawal_plan_with_locks, filter_allocation_plan, filter_unlocked_targets, MarketLock,
    MarketLockSet,
};
