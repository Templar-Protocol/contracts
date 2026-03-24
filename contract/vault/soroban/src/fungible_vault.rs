//! ERC-4626 / SEP-56 FungibleVault helpers for the Templar Soroban vault.
//!
//! Contains conversion helpers and atomic withdrawal logic used by the
//! `#[contractimpl]` block in `contract.rs`. The `#[contractimpl]` must live
//! in the same module as the struct definition to avoid Soroban macro conflicts.
//!
//! **Key differences from a vanilla ERC-4626 vault:**
//!
//! - `total_assets` includes external (market-deployed) assets tracked by the kernel.
//! - `withdraw` / `redeem` are **atomic from idle assets only**: they require the
//!   vault to be in `Idle` state and sufficient `idle_assets`. For the general case
//!   (assets deployed to markets), use `request_withdraw` + `execute_withdraw`.
//! - Conversion math uses the kernel's `effective_totals` formula which includes
//!   configurable `virtual_shares` / `virtual_assets` for inflation-attack mitigation.

use soroban_sdk::{token, Address as SdkAddress, Env};
use templar_vault_kernel::state::queue::DEFAULT_COOLDOWN_NS;
use templar_vault_kernel::{FeesSpec, VaultConfig, VaultState, MAX_PENDING, MIN_WITHDRAWAL_ASSETS};

use crate::contract::{get_config_address, VaultDataKey};
use crate::convert::runtime_to_contract;
use crate::error::ContractError;
use crate::storage::{SorobanStorage, Storage};

/// Load kernel state and a default config for read-only conversion math.
pub(crate) fn load_state_and_config(env: &Env) -> Result<(VaultState, VaultConfig), ContractError> {
    let storage = SorobanStorage::new(env);
    let state = storage.load_state();
    let state = runtime_to_contract(state)?
        .map(|v| v.state)
        .unwrap_or_default();
    let config = VaultConfig {
        fees: FeesSpec::zero(),
        min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
        withdrawal_cooldown_ns: DEFAULT_COOLDOWN_NS,
        max_pending_withdrawals: MAX_PENDING as u32,
        paused: storage.is_paused(),
        virtual_shares: 0,
        virtual_assets: 0,
    };
    Ok((state, config))
}

/// Read the share token balance for an address.
pub(crate) fn share_balance(env: &Env, owner: &SdkAddress) -> i128 {
    let share_token: SdkAddress = match get_config_address(env, &VaultDataKey::ShareToken) {
        Ok(addr) => addr,
        Err(_) => return 0,
    };
    token::Client::new(env, &share_token).balance(owner)
}
