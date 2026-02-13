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

use soroban_sdk::{token, Address as SdkAddress, Bytes, Env};
use soroban_sdk::token::StellarAssetClient;
use templar_vault_kernel::state::queue::DEFAULT_COOLDOWN_NS;
use templar_vault_kernel::{
    compute_fee_shares_from_assets, compute_management_fee_shares, total_assets_for_fee_accrual,
    FeeAccrualAnchor, FeesSpec, Number, VaultConfig, VaultState, MAX_PENDING,
    MIN_WITHDRAWAL_ASSETS,
};
use templar_vault_kernel::effects::KernelEvent;

use crate::contract::{get_config_address, load_fees_spec, VaultDataKey};
use crate::effects::KernelEventEnvelope;
use crate::error::ContractError;
use crate::storage::{SorobanStorage, Storage};

/// Load kernel state and a default config for read-only conversion math.
pub(crate) fn load_state_and_config(env: &Env) -> Result<(VaultState, VaultConfig), ContractError> {
    let storage = SorobanStorage::new(env);
    let state = storage
        .load_state()
        .map_err(ContractError::from)?
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

fn ledger_timestamp_ns(env: &Env) -> Result<u64, ContractError> {
    env.ledger()
        .timestamp()
        .checked_mul(1_000_000_000)
        .ok_or(ContractError::ConversionOverflow)
}

fn emit_kernel_event(env: &Env, event: &KernelEvent) -> Result<(), ContractError> {
    let payload =
        borsh::to_vec(event).map_err(|_| ContractError::EffectFailed)?;
    KernelEventEnvelope {
        payload: Bytes::from_slice(env, &payload),
    }
    .publish(env);
    Ok(())
}

fn resolve_fee_recipient(
    storage: &SorobanStorage,
    kernel_addr: &templar_vault_kernel::Address,
) -> Result<SdkAddress, ContractError> {
    storage
        .load_address(kernel_addr)
        .ok_or(ContractError::EffectFailed)
}

pub(crate) fn refresh_fees_for_atomic(env: &Env) -> Result<(), ContractError> {
    let now_ns = ledger_timestamp_ns(env)?;
    let mut storage = SorobanStorage::new(env);
    let mut versioned = storage
        .load_state()
        .map_err(ContractError::from)?
        .ok_or(ContractError::InvalidState)?;
    let state = &mut versioned.state;
    let anchor = state.fee_anchor;
    if now_ns < anchor.timestamp_ns {
        return Err(ContractError::InvalidState);
    }

    let fees = load_fees_spec(env).map_err(ContractError::from)?;
    let cur_total_assets = state.total_assets;
    let mut total_supply = state.total_shares;

    let fee_total_assets = total_assets_for_fee_accrual(
        cur_total_assets,
        anchor.total_assets,
        anchor.timestamp_ns,
        now_ns,
        fees.max_total_assets_growth_rate,
    );

    let share_token: SdkAddress = get_config_address(env, &VaultDataKey::ShareToken)?;
    let share_admin = StellarAssetClient::new(env, &share_token);

    let management_shares = compute_management_fee_shares(
        fee_total_assets,
        cur_total_assets,
        total_supply,
        fees.management.fee_wad,
        anchor.timestamp_ns,
        now_ns,
    );
    if !management_shares.is_zero() {
        let management_shares_u128: u128 = management_shares.into();
        let recipient = resolve_fee_recipient(&storage, &fees.management.recipient)?;
        let shares_i128 = to_i128(management_shares_u128)?;
        share_admin.mint(&recipient, &shares_i128);
        total_supply = total_supply
            .checked_add(management_shares_u128)
            .ok_or(ContractError::InvalidState)?;
    }

    let profit = fee_total_assets.saturating_sub(anchor.total_assets);
    let fee_assets = fees
        .performance
        .fee_wad
        .apply_floored(Number::from(profit));
    let performance_shares = compute_fee_shares_from_assets(
        fee_assets,
        Number::from(cur_total_assets),
        Number::from(total_supply),
    );
    if !performance_shares.is_zero() {
        let performance_shares_u128: u128 = performance_shares.into();
        let recipient = resolve_fee_recipient(&storage, &fees.performance.recipient)?;
        let shares_i128 = to_i128(performance_shares_u128)?;
        share_admin.mint(&recipient, &shares_i128);
        total_supply = total_supply
            .checked_add(performance_shares_u128)
            .ok_or(ContractError::InvalidState)?;
    }

    state.total_shares = total_supply;
    state.fee_anchor = FeeAccrualAnchor::new(cur_total_assets, now_ns);

    storage
        .save_state(&versioned)
        .map_err(ContractError::from)?;
    emit_kernel_event(
        env,
        &KernelEvent::FeesRefreshed {
            now_ns,
            total_assets: cur_total_assets,
        },
    )?;

    Ok(())
}

/// Read the share token balance for an address.
pub(crate) fn share_balance(env: &Env, owner: &SdkAddress) -> i128 {
    let share_token: SdkAddress = match get_config_address(env, &VaultDataKey::ShareToken) {
        Ok(addr) => addr,
        Err(_) => return 0,
    };
    token::Client::new(env, &share_token).balance(owner)
}

/// Safe u128 → i128 conversion.
pub(crate) fn to_i128(v: u128) -> Result<i128, ContractError> {
    i128::try_from(v).map_err(|_| ContractError::ConversionOverflow)
}

/// Safe i128 → u128 conversion (rejects negative).
pub(crate) fn to_u128(v: i128) -> Result<u128, ContractError> {
    if v < 0 {
        return Err(ContractError::InvalidInput);
    }
    Ok(v as u128)
}

/// Perform an atomic withdrawal by directly updating kernel state
/// and transferring tokens.
///
/// This bypasses the withdrawal queue and is only valid when Idle with
/// sufficient idle assets (caller must verify these preconditions).
pub(crate) fn atomic_withdraw_internal(
    env: &Env,
    owner: &SdkAddress,
    receiver: &SdkAddress,
    assets: u128,
    shares: u128,
) -> Result<(), ContractError> {
    let mut storage = SorobanStorage::new(env);

    // Load and mutate kernel state
    let mut versioned = storage
        .load_state()
        .map_err(ContractError::from)?
        .ok_or(ContractError::InvalidState)?;
    let state = &mut versioned.state;

    // Update kernel state totals
    state.total_shares = state
        .total_shares
        .checked_sub(shares)
        .ok_or(ContractError::InvalidState)?;
    state.total_assets = state
        .total_assets
        .checked_sub(assets)
        .ok_or(ContractError::InvalidState)?;
    state.idle_assets = state
        .idle_assets
        .checked_sub(assets)
        .ok_or(ContractError::InvalidState)?;

    // Persist updated state
    storage
        .save_state(&versioned)
        .map_err(ContractError::from)?;

    // Burn shares from owner via share token contract
    let share_token: SdkAddress = get_config_address(env, &VaultDataKey::ShareToken)?;
    let shares_i128 = to_i128(shares)?;
    let share_client = token::Client::new(env, &share_token);
    share_client.burn(owner, &shares_i128);

    // Transfer underlying assets to receiver
    let asset_token: SdkAddress = get_config_address(env, &VaultDataKey::AssetToken)?;
    let assets_i128 = to_i128(assets)?;
    let asset_client = token::Client::new(env, &asset_token);
    asset_client.transfer(&env.current_contract_address(), receiver, &assets_i128);

    Ok(())
}
