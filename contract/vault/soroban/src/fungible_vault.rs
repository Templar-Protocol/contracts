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
use templar_vault_kernel::{
    compute_fee_shares_from_assets, compute_management_fee_shares, total_assets_for_fee_accrual,
    FeeAccrualAnchor, Number, TimestampNs, VaultConfig, VaultState, MAX_PENDING,
    MIN_WITHDRAWAL_ASSETS,
};

use crate::contract::{load_fees_spec, load_virtual_offsets, VaultDataKey};
use crate::convert::{ledger_timestamp_ns, runtime_to_contract, to_u128};
use crate::error::ContractError;
use crate::storage::{SorobanStorage, Storage};

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
    if anchor.is_uninitialized() {
        state.fee_anchor = FeeAccrualAnchor::new(state.total_assets, TimestampNs(now_ns));
        return Ok(state);
    }
    if now_ns <= anchor.timestamp_ns.as_u64() {
        return Ok(state);
    }

    let current_assets = state.total_assets;
    let fee_assets_base = total_assets_for_fee_accrual(
        current_assets,
        anchor.total_assets,
        anchor.timestamp_ns.as_u64(),
        now_ns,
        config.fees.max_total_assets_growth_rate,
    );

    let management_shares = compute_management_fee_shares(
        fee_assets_base,
        current_assets,
        state.total_shares,
        config.fees.management.fee_wad,
        anchor.timestamp_ns.as_u64(),
        now_ns,
    );
    let max_supply = Number::from(u128::MAX);
    let supply_after_management =
        Number::from(state.total_shares).saturating_add(management_shares);
    if supply_after_management > max_supply {
        return Err(ContractError::ConversionOverflow);
    }

    let profit = fee_assets_base.saturating_sub(anchor.total_assets);
    let performance_fee_assets = config
        .fees
        .performance
        .fee_wad
        .apply_floored(Number::from(profit));
    let performance_shares = compute_fee_shares_from_assets(
        performance_fee_assets,
        Number::from(current_assets),
        supply_after_management,
    );

    let total_supply = supply_after_management.saturating_add(performance_shares);
    if total_supply > max_supply {
        return Err(ContractError::ConversionOverflow);
    }
    state.total_shares = total_supply.as_u128_trunc();
    state.fee_anchor = FeeAccrualAnchor::new(current_assets, TimestampNs(now_ns));

    Ok(state)
}

fn load_actual_idle_assets(env: &Env) -> Result<u128, ContractError> {
    let asset_token: Option<SdkAddress> = env.storage().instance().get(&VaultDataKey::AssetToken);
    let Some(asset_token) = asset_token else {
        return Ok(0);
    };
    to_u128(token::Client::new(env, &asset_token).balance(&env.current_contract_address()))
}

pub(crate) fn reconcile_actual_idle_assets(
    state: &mut VaultState,
    actual_idle_assets: u128,
    now_ns: u64,
) {
    if !state.is_idle() || state.idle_assets == actual_idle_assets {
        return;
    }

    state.idle_assets = actual_idle_assets;
    state.sync_total_assets();
    let observed_at = TimestampNs(now_ns);
    state.fee_anchor = FeeAccrualAnchor::new(state.total_assets, observed_at);
}

/// Load kernel state and a default config for read-only conversion math.
pub(crate) fn load_state_and_config(env: &Env) -> Result<(VaultState, VaultConfig), ContractError> {
    let storage = SorobanStorage::new(env);
    let stored_state = storage.load_state();
    let mut state = runtime_to_contract(stored_state)?.unwrap_or_default();
    let now_ns = ledger_timestamp_ns(env)?;
    let actual_idle_assets = load_actual_idle_assets(env)?;
    reconcile_actual_idle_assets(&mut state, actual_idle_assets, now_ns);
    let (virtual_shares, virtual_assets) = load_virtual_offsets(env);
    let config = VaultConfig {
        fees: runtime_to_contract(load_fees_spec(env))?,
        min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
        withdrawal_cooldown_ns: DEFAULT_COOLDOWN_NS,
        max_pending_withdrawals: MAX_PENDING as u32,
        paused: storage.is_paused(),
        virtual_shares,
        virtual_assets,
    };
    let fee_aware_state = preview_state_with_fee_accrual(env, state, &config)?;
    Ok((fee_aware_state, config))
}

/// Read the share token balance for an address.
pub(crate) fn share_balance(env: &Env, owner: &SdkAddress) -> i128 {
    let share_token: Option<SdkAddress> = env.storage().instance().get(&VaultDataKey::ShareToken);
    let Some(share_token) = share_token else {
        return 0;
    };
    token::Client::new(env, &share_token).balance(owner)
}
