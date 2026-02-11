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
pub mod reconciliation;
pub mod share_token;
pub mod storage;

// Re-exports for convenience
pub use auth::{ActionKind, AuthAdapter, AuthError, SorobanAuth};
pub use contract::{
    AllocationResult, ContractConfig, CuratorVault, DepositResult, RefreshResult,
    SorobanVaultContract, VaultDataKey, WithdrawRequestResult,
};
pub use effects::{
    AddressMap, AddressRegistrar, AllocDoneEvent, AllocStartEvent, AllocStepFailEvent,
    DepositEvent, EffectContext, EffectInterpreter, EffectResult, EffectSummary,
    ExtAssetsSyncEvent, FeesRefreshEvent, MockInterpreter, PauseUpdatedEvent, PayoutEvent,
    RefreshDoneEvent, RefreshStartEvent, SdkTokenAdapter, Sep41Operation, Sep41Token,
    SorobanEffectInterpreter, TestSep41Token, WithdrawCollectedEvent, WithdrawRequestEvent,
    WithdrawStartEvent, WithdrawStoppedEvent,
};
pub use error::{ContractError, RuntimeError};
pub use market::{
    AttemptId, CrossChainMarketAdapter, MarketAdapter, MarketRef, SettlementReceipt,
    SorobanCrossChainMarketAdapter, SorobanMarketAdapter, TestCrossChainAdapter, TestMarketAdapter,
};
pub use reconciliation::{
    build_refresh_plan, reconcile_external_assets, resync_external_assets, ReconciliationEvent,
    ReconciliationRecord, ResyncRequest, ResyncResult,
};
pub use storage::{
    MemoryStorage, SorobanStorage, SorobanStorageKey, SorobanVaultState, Storage, StorageVersion,
    VersionedState,
};
pub use templar_curator_primitives::rbac::{RbacAuth, RbacConfig, Role, RoleAssignment};

pub mod rbac {
    pub use templar_curator_primitives::rbac::{RbacAuth, RbacConfig, Role, RoleAssignment};
}

// Re-export soroban-sdk types for convenience
pub use soroban_sdk::{Address, Bytes, Env};

// Policy re-exports for convenience
pub use policy::{
    build_allocation_plan_with_locks, build_refresh_plan_with_locks,
    build_withdrawal_plan_with_locks, filter_allocation_plan, filter_unlocked_targets, MarketLock,
    MarketLockSet,
};
