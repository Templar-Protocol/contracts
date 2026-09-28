//! ERC-4626 / SEP-56 FungibleVault helpers for the Templar Soroban vault.
//!
//! Contains conversion helpers and atomic withdrawal logic used by the
//! `#[contractimpl]` block in `contract.rs`. The `#[contractimpl]` must live
//! in the same module as the struct definition to avoid Soroban macro conflicts.

use soroban_sdk::{token, Address as SdkAddress, Env};
use templar_vault_kernel::{
    compute_fee_accrual, should_refresh_fees_for_value_transfer, FeeAccrualAnchor, TimestampNs,
    VaultConfig, VaultState, MIN_WITHDRAWAL_ASSETS,
};

use crate::contract::{
    load_fees_spec, load_virtual_offsets, load_withdrawal_cooldown_ns, VaultDataKey,
};
use crate::convert::{ledger_timestamp_ns, runtime_to_contract, to_u128};
use crate::error::ContractError;
use crate::storage::{SorobanStorage, Storage, SOROBAN_MAX_PENDING_WITHDRAWALS};

fn preview_state_with_fee_accrual(
    env: &Env,
    mut state: VaultState,
    config: &VaultConfig,
) -> Result<VaultState, ContractError> {
    let now_ns = ledger_timestamp_ns(env)?;
    if state.total_shares == 0 {
        return Ok(state);
    }

    let anchor = state.fee_anchor;
    if anchor.is_uninitialized() && state.total_assets == 0 {
        state.fee_anchor = FeeAccrualAnchor::new(state.total_assets, TimestampNs(now_ns));
        return Ok(state);
    }
    if !should_refresh_fees_for_value_transfer(&state, config, TimestampNs(now_ns)) {
        return Ok(state);
    }

    let accrual = compute_fee_accrual(
        state.total_assets,
        state.total_shares,
        anchor,
        &config.fees,
        TimestampNs(now_ns),
    )
    .map_err(|_| ContractError::ConversionOverflow)?;
    state.total_shares = accrual.new_total_shares;
    state.fee_anchor = accrual.new_anchor;

    Ok(state)
}

fn load_actual_idle_assets(env: &Env) -> Result<u128, ContractError> {
    let asset_token: Option<SdkAddress> = env.storage().instance().get(&VaultDataKey::AssetToken);
    let Some(asset_token) = asset_token else {
        return Ok(0);
    };
    to_u128(token::Client::new(env, &asset_token).balance(&env.current_contract_address()))
}

pub(crate) fn share_balance(env: &Env, owner: &SdkAddress) -> i128 {
    let share_token: Option<SdkAddress> = env.storage().instance().get(&VaultDataKey::ShareToken);
    let Some(share_token) = share_token else {
        return 0;
    };
    token::Client::new(env, &share_token).balance(owner)
}

pub(crate) fn reconcile_actual_idle_assets(
    state: &mut VaultState,
    actual_idle_assets: u128,
) -> bool {
    if !state.is_idle() || state.idle_assets == actual_idle_assets {
        return false;
    }
    let inflow = actual_idle_assets.saturating_sub(state.idle_assets);
    state.idle_assets = actual_idle_assets;
    state.sync_total_assets();
    if inflow != 0 {
        state.fee_anchor.total_assets = state.fee_anchor.total_assets.saturating_add(inflow);
    }
    true
}

/// Load kernel state and a default config for read-only conversion math.
///
/// This helper feeds the Soroban view surface. Its values intentionally differ
/// from a vanilla ERC-4626 vault in two ways:
///
/// - `total_assets` includes market-deployed external assets tracked by the
///   kernel, while atomic `withdraw` / `redeem` can only consume `idle_assets`.
/// - Conversion math uses the kernel's `effective_totals` formula, including
///   configurable `virtual_shares` / `virtual_assets` for inflation-attack
///   mitigation.
///
/// Public view methods that report atomic withdrawal capacity must continue to
/// bound user exits by idle liquidity, not by total managed assets.
pub(crate) fn load_state_and_config(env: &Env) -> Result<(VaultState, VaultConfig), ContractError> {
    let storage = SorobanStorage::new(env);
    let stored_state = storage.load_state();
    let state = runtime_to_contract(stored_state)?.unwrap_or_default();
    let (virtual_shares, virtual_assets) = load_virtual_offsets(env);
    let config = VaultConfig {
        fees: runtime_to_contract(load_fees_spec(env))?,
        min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
        withdrawal_cooldown_ns: load_withdrawal_cooldown_ns(env),
        max_pending_withdrawals: SOROBAN_MAX_PENDING_WITHDRAWALS,
        paused: storage.is_paused(),
        virtual_shares,
        virtual_assets,
    };
    let mut fee_aware_state = preview_state_with_fee_accrual(env, state, &config)?;
    let actual_idle_assets = load_actual_idle_assets(env)?;
    reconcile_actual_idle_assets(&mut fee_aware_state, actual_idle_assets);
    Ok((fee_aware_state, config))
}
