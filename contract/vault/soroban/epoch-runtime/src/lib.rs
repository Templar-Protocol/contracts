//! Dedicated Soroban custodial epoch-settlement vault deploy target.
//!
//! This is a thin front-end package with its own contract package, version,
//! and capability identity. It links `templar-soroban-runtime` with
//! `default-features = false` and only the opt-in `epoch` feature, so the
//! compiled law is exactly:
//!
//! - epoch intake custody, cutoff, report-bound settlement, admission, and
//!   cancellation (pending deposit/withdrawal queue law), and
//! - the generic lifecycle entrypoints that are needed by a fresh deployment
//!   (initialize/migrate/execute/governance/upgrade).
//!
//! All lifecycle behavior is served through the runtime's non-exported
//! `epoch_law` entry API, so this package is the only crate that contributes
//! deployed contract exports. The immediate deposit/atomic-exit/pricing
//! entrypoints and proxy views of the default `templar-soroban-runtime`
//! product are compiled out of this artifact entirely: the epoch feature
//! never enables `immediate-entrypoints` here, so `VaultCommand` tags 0 and
//! 3-10 do not decode (`InvalidTag` on the wire, `InvalidInput` through
//! `execute`) and no proxy view or atomic exit callable exists in this
//! deployment. Each deployment of this WASM is a fresh contract instance with
//! its own storage; it shares no state with the immediate vault runtime.
//!
//! The callable `version()` entrypoint reports this package's version and the
//! epoch capability mask (`RUNTIME_EPOCH_FEATURE_FLAGS`), never the default
//! runtime identity.

#![no_std]

use soroban_sdk::{contract, contractimpl, Address, Bytes, BytesN, Env, String};
use templar_soroban_runtime::{epoch_law, error::ContractError};
use templar_soroban_shared_types::RUNTIME_EPOCH_FEATURE_FLAGS;

#[contract]
pub struct SorobanEpochVaultContract;

#[contractimpl]
impl SorobanEpochVaultContract {
    /// Return this epoch deploy target's package version and compiled
    /// capabilities. The mask is the dedicated epoch settlement mask
    /// (`action-pause | action-epoch-settlement`) and never advertises an
    /// immediate action or companion-upgrade capability.
    pub fn version(env: Env) -> (String, u64) {
        (
            String::from_str(&env, env!("CARGO_PKG_VERSION")),
            RUNTIME_EPOCH_FEATURE_FLAGS,
        )
    }

    pub fn initialize(
        env: Env,
        curator: Address,
        governance: Address,
        asset_token: Address,
        share_token: Address,
        virtual_shares: i128,
        virtual_assets: i128,
    ) -> Result<(), ContractError> {
        epoch_law::initialize(
            env,
            curator,
            governance,
            asset_token,
            share_token,
            virtual_shares,
            virtual_assets,
        )
    }

    pub fn initialize_with_config(
        env: Env,
        curator: Address,
        governance: Address,
        asset_token: Address,
        share_token: Address,
        virtual_shares: i128,
        virtual_assets: i128,
        withdrawal_cooldown_ns: u64,
    ) -> Result<(), ContractError> {
        epoch_law::initialize_with_config(
            env,
            curator,
            governance,
            asset_token,
            share_token,
            virtual_shares,
            virtual_assets,
            withdrawal_cooldown_ns,
        )
    }

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
    ) -> Result<(), ContractError> {
        epoch_law::initialize_with_full_config(
            env,
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

    /// Decode and execute one epoch-compatible vault command through the
    /// runtime epoch law. Immediate command payloads never decode in this
    /// build and are rejected with `InvalidInput`.
    pub fn execute(env: Env, payload: Bytes) -> Result<Bytes, ContractError> {
        epoch_law::execute(env, payload)
    }

    pub fn execute_governance(
        env: Env,
        caller: Address,
        payload: Bytes,
    ) -> Result<(), ContractError> {
        epoch_law::execute_governance(env, caller, payload)
    }

    pub fn upgrade(
        env: Env,
        new_wasm_hash: BytesN<32>,
        operator: Address,
    ) -> Result<(), ContractError> {
        epoch_law::upgrade(env, new_wasm_hash, operator)
    }

    pub fn migrate(env: Env, operator: Address) -> Result<(), ContractError> {
        epoch_law::migrate(env, operator)
    }
}
