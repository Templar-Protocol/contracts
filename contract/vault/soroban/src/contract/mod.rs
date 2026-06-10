//! Soroban curator vault contract entrypoints.
//!
//! This module provides the contract entrypoints that map to kernel actions.
//! Each entrypoint performs authorization, dispatches to kernel transitions,
//! and executes the returned effects.
//!
//! ## Soroban Contract
//!
//! The [`SorobanVaultContract`] provides the Soroban-native contract interface
//! with `#[contract]` and `#[contractimpl]` macros for deployment on the
//! Stellar network.

mod curator_vault;
mod entrypoints;
pub(crate) mod helpers;
mod types;

pub use curator_vault::CuratorVault;
pub use entrypoints::SorobanVaultContract;
pub use types::*;

use crate::auth::{ActionKind, AuthAdapter};
use crate::convert::{ledger_timestamp_ns, runtime_to_contract, to_i128, to_u128};
use crate::effects::{
    AddressRegistrar, EffectContext, EffectInterpreter, EffectSummary, SdkTokenAdapter,
    ShareTokenAdapter, SorobanEffectInterpreter,
};
use crate::error::{ContractError, RuntimeError};
use crate::fungible_vault::{load_state_and_config, reconcile_actual_idle_assets, share_balance};
use crate::market::{invoke_progress_withdrawal, invoke_supply, invoke_total_assets};
use crate::storage::{SorobanStorage, Storage, DEFAULT_TTL_EXTEND_TO, DEFAULT_TTL_THRESHOLD};
use alloc::string::String as AllocString;
use alloc::vec;
use alloc::vec::Vec;
use core::mem;
pub(crate) use helpers::*;
use soroban_sdk::{
    contract, contractimpl, symbol_short, Address as SdkAddress, Bytes, BytesN, Env, Executable,
};
use templar_curator_primitives::governance::TimelockDecision;
use templar_curator_primitives::policy::cap_group::{CapGroupId, CapGroupRecord, CapGroupUpdate};
use templar_curator_primitives::policy::state::MarketConfig;
use templar_curator_primitives::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
use templar_curator_primitives::rbac::{RbacAuth, RbacConfig, Role};
use templar_curator_primitives::PolicyState;
use templar_soroban_shared_types::VaultCommand;
use templar_vault_kernel::effects::KernelEffect;
use templar_vault_kernel::error::InvalidStateCode;
use templar_vault_kernel::{
    apply_action, convert_to_assets, convert_to_assets_bounded, convert_to_assets_ceil_bounded,
    convert_to_shares, convert_to_shares_bounded, convert_to_shares_ceil_bounded, plan_idle_payout,
    withdrawal_settled, Address, FeeAccrualAnchor, FeeSlot, FeesSpec, KernelAction, KernelResult,
    OpState, PayoutOutcome, Restrictions, TargetId, TimestampNs, VaultConfig, VaultState, Wad,
    DEFAULT_COOLDOWN_NS, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD, MIN_WITHDRAWAL_ASSETS,
};

use crate::storage::SOROBAN_MAX_PENDING_WITHDRAWALS;

pub(crate) const KERNEL_ADDRESS_DOMAIN: &[u8] = b"templar:soroban:address";
pub const SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS: u64 = DEFAULT_COOLDOWN_NS;
pub const SOROBAN_DEFAULT_IDLE_RESYNC_COOLDOWN_NS: u64 = 120 * 1_000_000_000;
const MIGRATION_FLAG_KEY: soroban_sdk::Symbol = symbol_short!("migrate");

pub(crate) fn decode_command(payload: &Bytes) -> Result<VaultCommand, ContractError> {
    VaultCommand::decode(&payload.to_alloc_vec()).map_err(|_| ContractError::InvalidInput)
}

pub(crate) fn encode_receipt(env: &Env, bytes: &[u8]) -> Bytes {
    Bytes::from_slice(env, bytes)
}

pub(crate) type ContractVault<'a> = CuratorVault<
    SorobanStorage<'a>,
    RbacAuth,
    SorobanEffectInterpreter<'a, ShareTokenAdapter<'a>, SdkTokenAdapter<'a>>,
>;
