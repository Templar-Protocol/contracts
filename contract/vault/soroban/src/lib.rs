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
//! 4. Interprets returned [`KernelEffect`](templar_vault_kernel::effects::KernelEffect)s via [`EffectInterpreter`]
//! 5. Persists state via [`Storage`]
//!
//! # Feature Flags
//!
//! The default runtime (`immediate-entrypoints`, on by default) compiles the
//! origin/dev immediate product exactly: the five production kernel action
//! gates (recovery, external synchronization, fee refresh, allocation
//! lifecycle, and refresh lifecycle) plus immediate deposit and atomic exit,
//! with the legacy queue schema and no epoch law or storage. The public
//! governance pause path is always available; `action-pause` separately
//! controls the kernel action variant.
//! The callable `version()` entrypoint reports the package version and exact
//! compiled runtime-capability mask. The reserved companion-upgrade capability
//! remains unset until the runtime can authorize companion-contract upgrades.
//!
//! The opt-in `epoch` feature (off by default) compiles the epoch settlement
//! library: pending-deposit intake custody, cutoff, report-bound settlement,
//! admission, and cancellation law. Combine with `immediate-entrypoints` to
//! retain guarded immediate entrypoints alongside settlement law. An
//! `epoch`-only build exposes no immediate commands and serves as the library
//! surface for the future thin epoch package.
//!
//! - `std` - Enable std library support (for testing)

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

#[cfg(not(feature = "epoch"))]
use templar_soroban_shared_types::{
    RUNTIME_FEATURE_ACTION_ALLOCATION_LIFECYCLE, RUNTIME_FEATURE_ACTION_PAUSE,
    RUNTIME_FEATURE_ACTION_RECOVERY, RUNTIME_FEATURE_ACTION_REFRESH_FEES,
    RUNTIME_FEATURE_ACTION_REFRESH_LIFECYCLE, RUNTIME_FEATURE_ACTION_SYNC_EXTERNAL,
};
#[cfg(feature = "epoch")]
use templar_soroban_shared_types::{
    RUNTIME_FEATURE_ACTION_ALLOCATION_LIFECYCLE, RUNTIME_FEATURE_ACTION_EPOCH_SETTLEMENT,
    RUNTIME_FEATURE_ACTION_PAUSE, RUNTIME_FEATURE_ACTION_RECOVERY,
    RUNTIME_FEATURE_ACTION_REFRESH_FEES, RUNTIME_FEATURE_ACTION_REFRESH_LIFECYCLE,
    RUNTIME_FEATURE_ACTION_SYNC_EXTERNAL,
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
#[cfg(not(feature = "epoch"))]
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
) | RUNTIME_FEATURE_ACTION_PAUSE;

/// Runtime capabilities compiled into this artifact.
#[cfg(feature = "epoch")]
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
    templar_vault_kernel::ACTION_EPOCH_SETTLEMENT_ENABLED,
    RUNTIME_FEATURE_ACTION_EPOCH_SETTLEMENT,
) | RUNTIME_FEATURE_ACTION_PAUSE;

// Capability mask law, enforced at compile time. The immediate product must
// expose exactly the six immediate capabilities (0x3F). The unified
// epoch+immediate profile must expose exactly those six plus epoch
// settlement (0xBF). The epoch-only library profile must expose exactly the
// pause and epoch settlement capabilities (0xA0) with no immediate action
// capability, because its kernel dependency carries no immediate action
// features. A build whose computed mask drifts from its profile law fails
// to compile instead of shipping a mis-advertised artifact.
#[cfg(not(feature = "epoch"))]
const _: () = assert!(RUNTIME_FEATURE_FLAGS == 0x3F);
#[cfg(all(feature = "epoch", feature = "immediate-entrypoints"))]
const _: () = assert!(RUNTIME_FEATURE_FLAGS == 0xBF);
#[cfg(all(feature = "epoch", not(feature = "immediate-entrypoints")))]
const _: () = assert!(RUNTIME_FEATURE_FLAGS == 0xA0);

pub mod auth;
pub(crate) mod convert;
pub mod market;

pub mod rbac {
    pub use templar_curator_primitives::rbac::{RbacAuth, RbacConfig, Role, RoleAssignment};
}

// Profile-exclusive law trees. The default immediate product compiles the
// origin/dev contract ABI, storage schema, events, and errors byte-for-byte
// from `src/immediate_law`. The opt-in `epoch` profile compiles the epoch
// settlement law from `src/contract`, `src/effects`, `src/error`,
// `src/fungible_vault`, and `src/storage`. Exactly one law tree is compiled
// per build, so a shipped immediate artifact contains no epoch code, keys,
// or events.
#[cfg(not(feature = "epoch"))]
#[path = "immediate_law/contract/mod.rs"]
pub mod contract;
#[cfg(not(feature = "epoch"))]
#[path = "immediate_law/effects.rs"]
pub mod effects;
#[cfg(not(feature = "epoch"))]
#[path = "immediate_law/error.rs"]
pub mod error;
#[cfg(not(feature = "epoch"))]
#[path = "immediate_law/fungible_vault.rs"]
pub mod fungible_vault;
#[cfg(not(feature = "epoch"))]
#[path = "immediate_law/storage.rs"]
pub mod storage;

#[cfg(feature = "epoch")]
pub mod contract;
#[cfg(feature = "epoch")]
pub mod effects;
#[cfg(feature = "epoch")]
pub mod error;
#[cfg(feature = "epoch")]
pub mod fungible_vault;
#[cfg(feature = "epoch")]
pub mod storage;


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


/// Non-exported epoch law entry API for the thin epoch package. These plain
/// functions forward to the exact same law the exported `#[contractimpl]`
/// surface forwards to. They carry no `#[contract]`, no `#[contractimpl]`, and
/// no `#[no_mangle]` export wrappers, so only the thin package contributes
/// deployed exports. Epoch feature only.
#[cfg(feature = "epoch")]
pub mod epoch_law {
    use soroban_sdk::{Address, Bytes, BytesN, Env};

    /// Return the package version and the exact capability mask compiled
    /// into the consuming artifact.
    pub fn version(env: Env) -> (soroban_sdk::String, u64) {
        crate::contract::version_law(&env)
    }

    /// Initialize a vault with default operational cooldowns.
    #[allow(clippy::too_many_arguments)]
    pub fn initialize(
        env: Env,
        curator: Address,
        governance: Address,
        asset_token: Address,
        share_token: Address,
        virtual_shares: i128,
        virtual_assets: i128,
    ) -> Result<(), crate::error::ContractError> {
        crate::contract::initialize_default_law(
            &env,
            curator,
            governance,
            asset_token,
            share_token,
            virtual_shares,
            virtual_assets,
        )
    }

    /// Initialize a vault with an explicit withdrawal cooldown.
    #[allow(clippy::too_many_arguments)]
    pub fn initialize_with_config(
        env: Env,
        curator: Address,
        governance: Address,
        asset_token: Address,
        share_token: Address,
        virtual_shares: i128,
        virtual_assets: i128,
        withdrawal_cooldown_ns: u64,
    ) -> Result<(), crate::error::ContractError> {
        crate::contract::initialize_with_config_law(
            &env,
            curator,
            governance,
            asset_token,
            share_token,
            virtual_shares,
            virtual_assets,
            withdrawal_cooldown_ns,
        )
    }

    /// Initialize a vault with every operational cooldown explicit.
    #[allow(clippy::too_many_arguments)]
    pub fn initialize_with_full_config(
        env: Env,
        curator: Address,
        governance: Address,
        asset_token: Address,
        share_token: Address,
        virtual_shares: i128,
        virtual_assets: i128,
        withdrawal_cooldown_ns: u64,
        idle_resync_cooldown_ns: u64,
    ) -> Result<(), crate::error::ContractError> {
        crate::contract::initialize_with_full_config_law(
            &env,
            curator,
            governance,
            asset_token,
            share_token,
            virtual_shares,
            virtual_assets,
            withdrawal_cooldown_ns,
            idle_resync_cooldown_ns,
        )
    }

    /// Decode and execute one vault command.
    pub fn execute(env: Env, payload: Bytes) -> Result<Bytes, crate::error::ContractError> {
        crate::contract::execute_payload_law(&env, &payload)
    }

    /// Decode and execute one governance command under authorization.
    pub fn execute_governance(
        env: Env,
        caller: Address,
        payload: Bytes,
    ) -> Result<(), crate::error::ContractError> {
        crate::contract::execute_governance_payload_law(&env, &caller, &payload)
    }

    /// Upgrade the deployed wasm under governance authority.
    pub fn upgrade(
        env: Env,
        new_wasm_hash: BytesN<32>,
        operator: Address,
    ) -> Result<(), crate::error::ContractError> {
        crate::contract::upgrade_law(&env, new_wasm_hash, &operator)
    }

    /// Complete a migration under governance authority.
    pub fn migrate(env: Env, operator: Address) -> Result<(), crate::error::ContractError> {
        crate::contract::migrate_law(&env, &operator)
    }
}

#[cfg(all(any(test, feature = "testutils"), not(feature = "epoch")))]
#[path = "immediate_law/test_utils.rs"]
pub mod test_utils;

#[cfg(all(any(test, feature = "testutils"), feature = "epoch"))]
pub mod test_utils;

#[cfg(all(test, not(feature = "epoch")))]
#[path = "immediate_law/tests.rs"]
mod tests;

// Epoch law unit tests drive the exported command surface and therefore
// compile only in the unified epoch+immediate profile. Pure epoch-profile
// capability and law evidence is produced by the thin epoch package's host
// suite against its exported contract, and the epoch-only library profile
// is proven here by compile-time mask law plus a zero-export optimized
// artifact check.
#[cfg(all(
    test,
    feature = "epoch",
    feature = "immediate-entrypoints"
))]
mod tests;
