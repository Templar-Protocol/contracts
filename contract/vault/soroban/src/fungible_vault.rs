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
use crate::convert::{ledger_timestamp_ns, runtime_to_contract};
use crate::error::ContractError;
use crate::storage::{SorobanStorage, Storage};

fn preview_state_with_fee_accrual(
    env: &Env,
    mut state: VaultState,
    config: &VaultConfig,
) -> Result<VaultState, ContractError> {
    let now_ns = ledger_timestamp_ns(env)?;
    let anchor = state.fee_anchor;

    if state.total_shares == 0 || now_ns <= anchor.timestamp_ns.as_u64() {
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
    let supply_after_management =
        Number::from(state.total_shares).saturating_add(management_shares);

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

    state.total_shares = supply_after_management
        .saturating_add(performance_shares)
        .as_u128_saturating();
    state.fee_anchor = FeeAccrualAnchor::new(current_assets, TimestampNs(now_ns));

    Ok(state)
}

/// Load kernel state and a default config for read-only conversion math.
pub(crate) fn load_state_and_config(env: &Env) -> Result<(VaultState, VaultConfig), ContractError> {
    let storage = SorobanStorage::new(env);
    let stored_state = storage.load_state();
    let state = runtime_to_contract(stored_state)?
        .map(|v| v.state)
        .unwrap_or_default();
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
