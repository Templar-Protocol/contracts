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
mod helpers;
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
use crate::fungible_vault::{load_state_and_config, share_balance};
use crate::market::{invoke_progress_withdrawal, invoke_supply, invoke_total_assets};
use crate::storage::{
    SorobanStorage, Storage, VersionedState, DEFAULT_TTL_EXTEND_TO, DEFAULT_TTL_THRESHOLD,
};
use alloc::string::String as AllocString;
use alloc::vec;
use alloc::vec::Vec;
use core::mem;
use core::num::NonZeroU128;
pub(crate) use helpers::*;
use soroban_sdk::{
    contract, contractimpl, symbol_short, Address as SdkAddress, Bytes, BytesN, Env,
};
use templar_curator_primitives::governance::TimelockDecision;
use templar_curator_primitives::policy::cap_group::{CapGroupId, CapGroupRecord, CapGroupUpdate};
use templar_curator_primitives::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
use templar_curator_primitives::rbac::{RbacAuth, RbacConfig, Role};
use templar_curator_primitives::PolicyState;
use templar_vault_kernel::actions::AtomicPayoutKind;
use templar_vault_kernel::effects::KernelEffect;
use templar_vault_kernel::state::queue::DEFAULT_COOLDOWN_NS;
use templar_vault_kernel::{
    apply_action, complete_allocation, compute_idle_settlement, convert_to_assets,
    convert_to_assets_ceil, convert_to_shares, convert_to_shares_ceil, start_allocation,
    withdrawal_settled, Address, FeeAccrualAnchor, FeeSlot, FeesSpec, KernelAction, OpState,
    PayoutOutcome, Restrictions, TargetId, VaultConfig, VaultState, Wad, MAX_MANAGEMENT_FEE_WAD,
    MAX_PENDING, MAX_PERFORMANCE_FEE_WAD, MIN_WITHDRAWAL_ASSETS,
};

const ESCROW_ADDRESS: Address = [0u8; 32];
pub(crate) const KERNEL_ADDRESS_DOMAIN: &[u8] = b"templar:soroban:address";
const MIGRATION_FLAG_KEY: soroban_sdk::Symbol = symbol_short!("migrate");

pub(crate) type ContractVault<'a> = CuratorVault<
    SorobanStorage<'a>,
    RbacAuth,
    SorobanEffectInterpreter<'a, ShareTokenAdapter<'a>, SdkTokenAdapter<'a>>,
>;
