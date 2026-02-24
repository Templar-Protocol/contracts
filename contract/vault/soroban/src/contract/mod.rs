//! Soroban curator vault contract entrypoints.
//!
//! This module provides the contract entrypoints that map to kernel actions.
//! Each entrypoint performs authorization, dispatches to kernel transitions,
//! and executes the returned effects.
//!
//! ## Soroban Contract
//!
//! The [`SorobanVaultContract`] at the bottom of this module provides the
//! Soroban-native contract interface with `#[contract]` and `#[contractimpl]`
//! macros for deployment on the Stellar network.

mod types;
pub use types::*;

use crate::convert::{ledger_timestamp_ns, runtime_to_contract, to_i128, to_u128};
use crate::fungible_vault::{
    atomic_withdraw_internal, load_state_and_config, refresh_fees_for_atomic, share_balance,
};
use alloc::string::String as AllocString;
use alloc::vec;
use alloc::vec::Vec;
use core::mem;
use core::num::NonZeroU128;
use soroban_sdk::{
    contract, contractimpl, symbol_short, Address as SdkAddress, Bytes, BytesN, Env, IntoVal,
    Symbol, Val,
};
use templar_curator_primitives::governance::{
    cap_change_decision, market_removal_decision, membership_change_decision,
    relative_cap_change_decision, TimelockDecision,
};
use templar_curator_primitives::policy::cap_group::{CapGroupId, CapGroupRecord, CapGroupUpdate};
use templar_curator_primitives::policy::state::MarketConfig;
use templar_curator_primitives::PolicyState;
use templar_vault_kernel::effects::KernelEffect;
use templar_vault_kernel::error::KernelError;
use templar_vault_kernel::state::queue::{compute_full_withdrawal, DEFAULT_COOLDOWN_NS};
use templar_vault_kernel::{
    apply_action, complete_allocation, complete_refresh, convert_to_assets, convert_to_assets_ceil,
    convert_to_shares, convert_to_shares_ceil, start_allocation, start_refresh,
    withdrawal_collected, withdrawal_step_callback, Address, FeeAccrualAnchor, FeeSlot, FeesSpec,
    KernelAction, OpState, PayoutOutcome, Restrictions, TargetId, TransitionError, VaultConfig,
    VaultState, Wad, MAX_MANAGEMENT_FEE_WAD, MAX_PENDING, MAX_PERFORMANCE_FEE_WAD,
    MIN_WITHDRAWAL_ASSETS,
};

use crate::auth::{ActionKind, AuthAdapter};
use crate::effects::{
    AddressRegistrar, EffectContext, EffectInterpreter, EffectSummary, SdkTokenAdapter,
    ShareTokenAdapter, SorobanEffectInterpreter,
};
use crate::error::{ContractError, RuntimeError};
use crate::policy::{build_refresh_plan_with_locks, SupplyQueue, SupplyQueueEntry};
use crate::storage::{SorobanStorage, Storage, VersionedState};
use templar_curator_primitives::rbac::{RbacAuth, RbacConfig, Role};

const ESCROW_ADDRESS: Address = [0u8; 32];
const KERNEL_ADDRESS_DOMAIN: &[u8] = b"templar:soroban:address";
use crate::storage::{DEFAULT_TTL_EXTEND_TO, DEFAULT_TTL_THRESHOLD};

/// Deterministic one-way mapping from Soroban address to kernel Address.
///
/// Uses a domain prefix so hashes do not collide with other chains' mappings.
fn kernel_address_from_sdk(env: &Env, addr: &SdkAddress) -> Address {
    let strkey = addr.to_string();
    let strkey_bytes = strkey.to_bytes();
    let mut strkey_vec = vec![0u8; strkey_bytes.len() as usize];
    strkey_bytes.copy_into_slice(&mut strkey_vec);
    let mut raw = Vec::with_capacity(KERNEL_ADDRESS_DOMAIN.len() + strkey_vec.len());
    raw.extend_from_slice(KERNEL_ADDRESS_DOMAIN);
    raw.extend_from_slice(&strkey_vec);
    let bytes = Bytes::from_slice(env, &raw);
    env.crypto().sha256(&bytes).to_bytes().to_array()
}

fn is_contract_address(addr: &SdkAddress) -> bool {
    let bytes = addr.to_string().to_bytes();
    matches!(bytes.get(0), Some(b'C'))
}

fn require_contract_address(addr: &SdkAddress) -> Result<(), ContractError> {
    if is_contract_address(addr) {
        Ok(())
    } else {
        Err(ContractError::InvalidInput)
    }
}

fn serialize_fees_spec(fees: &FeesSpec) -> Result<Vec<u8>, RuntimeError> {
    match postcard::to_allocvec(fees) {
        Ok(bytes) => Ok(bytes),
        Err(_) => Err(RuntimeError::storage_error("fees serialize failed")),
    }
}

fn deserialize_fees_spec(bytes: &[u8]) -> Result<FeesSpec, RuntimeError> {
    match postcard::from_bytes(bytes) {
        Ok(fees) => Ok(fees),
        Err(_) => Err(RuntimeError::storage_error("fees deserialize failed")),
    }
}

pub(crate) fn load_fees_spec(env: &Env) -> Result<FeesSpec, RuntimeError> {
    let stored: Option<Bytes> = env.storage().instance().get(&VaultDataKey::FeesSpec);
    match stored {
        Some(bytes) => deserialize_fees_spec(&bytes.to_alloc_vec()),
        None => Ok(FeesSpec::zero()),
    }
}

fn store_fees_spec(env: &Env, fees: &FeesSpec) -> Result<(), RuntimeError> {
    let bytes = serialize_fees_spec(fees)?;
    env.storage()
        .instance()
        .set(&VaultDataKey::FeesSpec, &Bytes::from_slice(env, &bytes));
    Ok(())
}

#[cold]
fn contract_error(msg: &'static str) -> RuntimeError {
    RuntimeError::contract_error(msg)
}

#[cold]
fn invalid_state_error(msg: &'static str) -> RuntimeError {
    RuntimeError::invalid_state(msg)
}

/// Curator vault contract.
///
/// This struct wraps the vault state, storage, auth, effect interpreter,
/// and market adapters to provide the full contract interface.
pub struct CuratorVault<S, A, E>
where
    S: Storage,
    A: AuthAdapter,
    E: EffectInterpreter + AddressRegistrar,
{
    /// Contract configuration.
    pub config: ContractConfig,
    /// Storage backend.
    pub storage: S,
    /// Auth adapter.
    pub auth: A,
    /// Effect interpreter.
    pub interpreter: E,
    /// Vault state (loaded from storage).
    state: Option<VaultState>,
    /// Policy state (locks, caps, supply queue).
    policy_state: PolicyState,
    /// Optional kernel restrictions (pause/allowlist/denylist).
    restrictions: Option<Restrictions>,
    /// Whether the vault is paused.
    paused: bool,
}

impl<S, A, E> CuratorVault<S, A, E>
where
    S: Storage,
    A: AuthAdapter,
    E: EffectInterpreter + AddressRegistrar,
{
    /// Create a new curator vault instance.
    #[inline]
    #[must_use]
    pub fn new(config: ContractConfig, storage: S, auth: A, interpreter: E) -> Self {
        Self {
            config,
            storage,
            auth,
            interpreter,
            state: None,
            policy_state: PolicyState::new(),
            restrictions: None,
            paused: false,
        }
    }

    #[inline(never)]
    pub fn load_state(&mut self) -> Result<(), RuntimeError> {
        match self.storage.load_state()? {
            Some(versioned) => {
                self.state = Some(versioned.state);
            }
            None => {
                self.state = Some(VaultState::default());
            }
        }
        self.paused = self.storage.load_paused()?;
        self.policy_state = self
            .storage
            .load_policy_state()?
            .unwrap_or_else(PolicyState::new);
        self.restrictions = self.storage.load_restrictions()?;
        Ok(())
    }

    /// Register a kernel address mapping for effect execution.
    ///
    /// Persists the mapping so follow-up calls can resolve addresses
    /// even when the Soroban address is not provided again.
    pub fn register_address(
        &mut self,
        kernel_addr: Address,
        soroban_addr: SdkAddress,
    ) -> Result<(), RuntimeError> {
        self.storage.save_address(&kernel_addr, &soroban_addr)?;
        self.interpreter.register_address(kernel_addr, soroban_addr);
        Ok(())
    }

    /// Save vault state to storage.
    pub fn save_state(&mut self) -> Result<(), RuntimeError> {
        if let Some(ref state) = self.state {
            let versioned = VersionedState::new(state.clone());
            self.storage.save_state(&versioned)?;
        }
        Ok(())
    }

    fn authorize(&self, kind: ActionKind, caller: Address) -> Result<(), RuntimeError> {
        self.auth.authorize(kind, caller, None)?;
        Ok(())
    }

    fn reserve_op_id(state: &mut VaultState) -> Result<u64, RuntimeError> {
        let op_id = state.next_op_id;
        state.next_op_id = match state.next_op_id.checked_add(1) {
            Some(next) => next,
            None => return Err(invalid_state_error("op_id overflow")),
        };
        Ok(op_id)
    }

    /// Get a reference to the current vault state.
    #[inline]
    pub fn state(&self) -> Result<&VaultState, RuntimeError> {
        match self.state.as_ref() {
            Some(state) => Ok(state),
            None => Err(RuntimeError::storage_error("vault state not loaded")),
        }
    }

    /// Get a mutable reference to the current vault state.
    #[inline]
    pub fn state_mut(&mut self) -> Result<&mut VaultState, RuntimeError> {
        match self.state.as_mut() {
            Some(state) => Ok(state),
            None => Err(RuntimeError::storage_error("vault state not loaded")),
        }
    }

    /// Build effect context from current state.
    fn effect_context(&self, now_ns: u64) -> EffectContext {
        EffectContext::new(
            now_ns,
            self.config.vault_address,
            self.config.asset_address,
            self.config.share_address,
        )
    }

    fn ensure_vault_mapped(&mut self, env: &Env) -> Result<(), RuntimeError> {
        let vault_sdk = env.current_contract_address();
        let vault_kernel = kernel_address_from_sdk(env, &vault_sdk);
        if vault_kernel != self.config.vault_address {
            return Err(RuntimeError::contract_error(
                "vault address mismatch for effect mapping",
            ));
        }
        self.register_address(vault_kernel, vault_sdk.clone())?;
        self.register_address(ESCROW_ADDRESS, vault_sdk)?;
        Ok(())
    }

    fn register_sdk_address(
        &mut self,
        env: &Env,
        addr: &SdkAddress,
    ) -> Result<Address, RuntimeError> {
        let kernel_addr = kernel_address_from_sdk(env, addr);
        self.register_address(kernel_addr, addr.clone())?;
        Ok(kernel_addr)
    }

    fn kernel_config(&self) -> VaultConfig {
        VaultConfig {
            fees: self.config.fees,
            min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
            withdrawal_cooldown_ns: DEFAULT_COOLDOWN_NS,
            max_pending_withdrawals: MAX_PENDING as u32,
            paused: self.paused,
            virtual_shares: 0,
            virtual_assets: 0,
        }
    }

    #[inline(never)]
    fn apply_kernel_action_local(
        &mut self,
        action: KernelAction,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        let config = self.kernel_config();
        let restrictions = self.restrictions.as_ref();
        let state = self
            .state
            .take()
            .ok_or_else(|| RuntimeError::storage_error("vault state not loaded"))?;
        let result = match kernel_to_runtime(apply_action(
            state.clone(),
            &config,
            restrictions,
            &self.config.vault_address,
            action,
        )) {
            Ok(r) => r,
            Err(e) => {
                self.state = Some(state);
                return Err(e);
            }
        };

        let ctx = self.effect_context(now_ns);
        if let Err(e) = self.ensure_effect_addresses_mapped(&result.effects, &ctx) {
            self.state = Some(result.state);
            return Err(e);
        }
        match self.interpreter.execute_effects(&result.effects, &ctx) {
            Ok(summary) => {
                self.state = Some(result.state);
                self.save_state()?;
                Ok(summary)
            }
            Err(e) => {
                self.state = Some(result.state);
                Err(e)
            }
        }
    }

    #[inline(never)]
    fn apply_kernel_action(
        &mut self,
        action: KernelAction,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        self.apply_kernel_action_local(action, now_ns)
    }

    #[inline(never)]
    fn ensure_effect_addresses_mapped(
        &mut self,
        effects: &[KernelEffect],
        ctx: &EffectContext,
    ) -> Result<(), RuntimeError> {
        for effect in effects {
            match effect {
                KernelEffect::MintShares { owner, .. } | KernelEffect::BurnShares { owner, .. } => {
                    self.ensure_mapped(owner)?;
                }
                KernelEffect::TransferShares { from, to, .. } => {
                    self.ensure_mapped(from)?;
                    self.ensure_mapped(to)?;
                }
                KernelEffect::TransferAssets { to, .. } => {
                    self.ensure_mapped(&ctx.vault_address)?;
                    self.ensure_mapped(to)?;
                }
                KernelEffect::TransferAssetsFrom { from, to, .. } => {
                    self.ensure_mapped(from)?;
                    self.ensure_mapped(to)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn ensure_mapped(&mut self, addr: &Address) -> Result<(), RuntimeError> {
        if self.interpreter.has_address(addr) {
            return Ok(());
        }
        if let Some(soroban_addr) = self.storage.load_address(addr)? {
            self.interpreter.register_address(*addr, soroban_addr);
            return Ok(());
        }
        Err(RuntimeError::effect_failed("missing address mapping"))
    }

    /// Deposit assets into the vault.
    #[inline(never)]
    pub fn deposit(
        &mut self,
        caller: Address,
        receiver: Address,
        assets: u128,
        min_shares_out: u128,
        now_ns: u64,
    ) -> Result<DepositResult, RuntimeError> {
        // Authorize
        self.auth.authorize(ActionKind::Deposit, caller, None)?;

        if self.paused {
            return Err(contract_error("vault is paused"));
        }

        let summary = self.apply_kernel_action(
            KernelAction::Deposit {
                owner: caller,
                receiver,
                assets_in: assets,
                min_shares_out,
                now_ns,
            },
            now_ns,
        )?;
        let shares = summary.shares_minted;

        let state = self.state()?;
        Ok(DepositResult {
            shares_minted: shares,
            total_shares: state.total_shares,
            total_assets: state.total_assets,
        })
    }

    #[inline(never)]
    pub fn deposit_soroban(
        &mut self,
        env: &Env,
        caller: SdkAddress,
        receiver: SdkAddress,
        assets: u128,
        min_shares_out: u128,
        now_ns: u64,
    ) -> Result<DepositResult, RuntimeError> {
        self.ensure_vault_mapped(env)?;
        let caller_kernel = self.register_sdk_address(env, &caller)?;
        let receiver_kernel = self.register_sdk_address(env, &receiver)?;
        self.deposit(
            caller_kernel,
            receiver_kernel,
            assets,
            min_shares_out,
            now_ns,
        )
    }

    /// Request a withdrawal from the vault.
    #[inline(never)]
    pub fn request_withdraw(
        &mut self,
        caller: Address,
        receiver: Address,
        shares: u128,
        min_assets_out: u128,
        now_ns: u64,
    ) -> Result<WithdrawRequestResult, RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::RequestWithdraw, caller, None)?;

        let state = self.state()?;
        if state.total_shares == 0 {
            return Err(contract_error("no shares in vault"));
        }

        let request_id = state.withdraw_queue.next_pending_withdrawal_id;

        let action = KernelAction::RequestWithdraw {
            owner: caller,
            receiver,
            shares,
            min_assets_out,
            now_ns,
        };
        let _summary = self.apply_kernel_action(action, now_ns)?;

        Ok(WithdrawRequestResult {
            request_id,
            shares_escrowed: shares,
        })
    }

    #[inline(never)]
    pub fn request_withdraw_soroban(
        &mut self,
        env: &Env,
        caller: SdkAddress,
        receiver: SdkAddress,
        shares: u128,
        min_assets_out: u128,
        now_ns: u64,
    ) -> Result<WithdrawRequestResult, RuntimeError> {
        self.ensure_vault_mapped(env)?;
        let caller_kernel = self.register_sdk_address(env, &caller)?;
        let receiver_kernel = self.register_sdk_address(env, &receiver)?;
        self.request_withdraw(
            caller_kernel,
            receiver_kernel,
            shares,
            min_assets_out,
            now_ns,
        )
    }

    /// Execute a pending withdrawal.
    #[inline(never)]
    pub fn execute_withdraw(
        &mut self,
        caller: Address,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::ExecuteWithdraw, caller, None)?;

        let mut summary = EffectSummary::new();

        {
            let op_state = &self.state()?.op_state;
            if !op_state.is_idle() && !op_state.is_withdrawing() {
                return Err(contract_error(
                    "vault not in idle or withdrawing state for withdrawal",
                ));
            }
        }
        if self.state()?.op_state.is_idle() {
            let step_summary =
                self.apply_kernel_action(KernelAction::ExecuteWithdraw { now_ns }, now_ns)?;
            summary.merge(step_summary);
        }

        if self.state()?.op_state.is_withdrawing() {
            let settle_summary = self.complete_withdrawal_from_idle(now_ns)?;
            summary.merge(settle_summary);
        }

        Ok(summary)
    }

    #[inline(never)]
    pub fn execute_withdraw_soroban(
        &mut self,
        env: &Env,
        caller: SdkAddress,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        self.ensure_vault_mapped(env)?;
        let caller_kernel = self.register_sdk_address(env, &caller)?;
        self.execute_withdraw(caller_kernel, now_ns)
    }

    #[inline(never)]
    fn complete_withdrawal_from_idle(
        &mut self,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        let (_, pending) = match self.state()?.withdraw_queue.head() {
            Some(entry) => entry,
            None => return Err(contract_error("withdraw queue empty")),
        };
        let pending = pending.clone();

        let withdraw = match &self.state()?.op_state {
            OpState::Withdrawing(state) => state.clone(),
            _ => return Err(contract_error("withdrawal not in progress")),
        };

        if pending.owner != withdraw.owner
            || pending.receiver != withdraw.receiver
            || pending.escrow_shares != withdraw.escrow_shares
        {
            return Err(contract_error("withdrawal queue head mismatch"));
        }

        let available_assets = self.state()?.idle_assets;
        if available_assets == 0 {
            return Ok(EffectSummary::new());
        }

        if available_assets < pending.expected_assets {
            return Ok(EffectSummary::new());
        }

        let withdrawal_result = match compute_full_withdrawal(&pending, available_assets) {
            Some(result) => result,
            None => return Err(contract_error("withdrawal not satisfiable")),
        };

        let assets_out = withdrawal_result.assets_out;
        if assets_out == 0 {
            return Ok(EffectSummary::new());
        }

        let burn_shares = withdrawal_result.settlement.to_burn;
        let refund_shares = withdrawal_result.settlement.refund;
        let op_id = withdraw.op_id;

        let step = {
            let op_state = mem::take(&mut self.state_mut()?.op_state);
            transition_to_runtime(withdrawal_step_callback(op_state, op_id, assets_out))?
        };
        self.state_mut()?.op_state = step.new_state;

        let collected = {
            let op_state = mem::take(&mut self.state_mut()?.op_state);
            transition_to_runtime(withdrawal_collected(op_state, op_id, burn_shares))?
        };
        let ctx = self.effect_context(now_ns);
        self.ensure_effect_addresses_mapped(&collected.effects, &ctx)?;
        let mut summary = self.interpreter.execute_effects(&collected.effects, &ctx)?;
        self.state_mut()?.op_state = collected.new_state;

        let payout = match &self.state()?.op_state {
            OpState::Payout(state) => state,
            _ => return Err(contract_error("expected payout state after withdrawal")),
        };

        let transfer_effects = [KernelEffect::TransferAssets {
            to: payout.receiver,
            amount: assets_out,
        }];
        self.ensure_effect_addresses_mapped(&transfer_effects, &ctx)?;
        let transfer_summary = self.interpreter.execute_effects(&transfer_effects, &ctx)?;
        summary.merge(transfer_summary);

        let state = self.state_mut()?;
        state.idle_assets = match state.idle_assets.checked_sub(assets_out) {
            Some(idle_assets) => idle_assets,
            None => return Err(invalid_state_error("idle_assets underflow on withdrawal")),
        };
        state.total_assets = match state.idle_assets.checked_add(state.external_assets) {
            Some(total_assets) => total_assets,
            None => return Err(invalid_state_error("total_assets overflow on withdrawal")),
        };

        let settle_summary = self.apply_kernel_action(
            KernelAction::SettlePayout {
                op_id,
                outcome: PayoutOutcome::Success {
                    burn_shares,
                    refund_shares,
                },
            },
            now_ns,
        )?;
        summary.merge(settle_summary);

        Ok(summary)
    }

    /// Pause or unpause the vault.
    ///
    pub fn pause(&mut self, caller: Address, paused: bool) -> Result<(), RuntimeError> {
        // Authorize
        self.auth.authorize(ActionKind::Pause, caller, None)?;

        self.paused = paused;
        self.storage.save_paused(paused)?;
        Ok(())
    }

    /// Set kernel restrictions for the vault.
    ///
    pub fn set_restrictions(
        &mut self,
        caller: Address,
        restrictions: Option<Restrictions>,
    ) -> Result<(), RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::SetRestrictions, caller, None)?;

        self.restrictions = restrictions;
        self.storage.save_restrictions(&self.restrictions)?;
        Ok(())
    }

    pub fn allocate(
        &mut self,
        caller: Address,
        delta: &AllocationDelta,
    ) -> Result<AllocationResult, RuntimeError> {
        match delta {
            AllocationDelta::Supply(d) => {
                if d.amount == 0 {
                    return Err(RuntimeError::invalid_input("amount must be > 0"));
                }

                let plan = vec![(d.market.into(), d.amount)];
                let op_id = self.begin_allocation_internal(caller, &plan)?;

                {
                    let state = self.state_mut()?;
                    state.external_assets = state.external_assets.saturating_add(d.amount);
                    state.sync_total_assets();
                }

                self.finish_allocation_internal(op_id)?;
                self.save_state()?;
                Ok(AllocationResult {
                    op_id,
                    new_external_assets: self.state()?.external_assets,
                    summary: EffectSummary::new(),
                })
            }
            AllocationDelta::Withdraw(d) => {
                if d.amount == 0 {
                    return Err(RuntimeError::invalid_input("amount must be > 0"));
                }

                self.authorize(ActionKind::BeginAllocating, caller)?;
                let new_external = {
                    let state = self.state_mut()?;
                    state.external_assets = state.external_assets.saturating_sub(d.amount);
                    state.idle_assets = state.idle_assets.saturating_add(d.amount);
                    state.sync_total_assets();
                    state.external_assets
                };

                self.save_state()?;
                Ok(AllocationResult {
                    op_id: 0,
                    new_external_assets: new_external,
                    summary: EffectSummary::new(),
                })
            }
        }
    }

    fn begin_allocation_internal(
        &mut self,
        caller: Address,
        plan: &[(TargetId, u128)],
    ) -> Result<u64, RuntimeError> {
        self.authorize(ActionKind::BeginAllocating, caller)?;
        let state = self.state_mut()?;
        let op_id = state.next_op_id;
        state.next_op_id = state.next_op_id.saturating_add(1);

        let result = transition_to_runtime(start_allocation(
            mem::take(&mut state.op_state),
            plan.to_vec(),
            op_id,
        ))?;

        let alloc_total: u128 = plan.iter().map(|(_, amt)| *amt).sum();
        if alloc_total > state.idle_assets {
            return Err(RuntimeError::insufficient_balance(
                state.idle_assets,
                alloc_total,
            ));
        }
        state.idle_assets -= alloc_total;
        state.sync_total_assets();
        state.op_state = result.new_state;

        Ok(op_id)
    }

    fn finish_allocation_internal(&mut self, op_id: u64) -> Result<(), RuntimeError> {
        let state = self.state_mut()?;
        let result = transition_to_runtime(complete_allocation(
            mem::take(&mut state.op_state),
            op_id,
            None,
        ))?;
        state.op_state = result.new_state;
        Ok(())
    }

    pub fn refresh_markets(
        &mut self,
        caller: Address,
        markets: Vec<TargetId>,
        now_ns: u64,
    ) -> Result<RefreshResult, RuntimeError> {
        let op_id = self.begin_refreshing(caller, markets, now_ns)?;
        self.finish_refreshing(caller, op_id)
    }

    // -------------------------------------------------------------------------
    // Test-only helpers (pub(crate) for unit/integration tests)
    // These expose the kernel state machine steps that `allocate` uses internally.
    // -------------------------------------------------------------------------

    /// Begin an allocation operation (test helper).
    ///
    /// Filters the plan to exclude locked markets, validates idle assets,
    /// and transitions the kernel to Allocating state.
    #[cfg(any(test, feature = "testutils"))]
    pub fn begin_allocating(
        &mut self,
        caller: Address,
        plan: Vec<(TargetId, u128)>,
        current_ns: u64,
    ) -> Result<u64, RuntimeError> {
        use crate::policy::filter_allocation_plan;

        // Filter plan to exclude locked markets
        let filtered_plan = filter_allocation_plan(&plan, &self.policy_state.locks, current_ns);

        self.authorize(ActionKind::BeginAllocating, caller)?;
        let op_id = {
            let state = self.state_mut()?;
            let op_id = state.next_op_id;
            state.next_op_id = state.next_op_id.saturating_add(1);

            let alloc_total: u128 = filtered_plan.iter().map(|(_, amt)| *amt).sum();
            if alloc_total > state.idle_assets {
                return Err(RuntimeError::insufficient_balance(
                    state.idle_assets,
                    alloc_total,
                ));
            }
            state.idle_assets -= alloc_total;
            state.sync_total_assets();

            let result = transition_to_runtime(start_allocation(
                mem::take(&mut state.op_state),
                filtered_plan,
                op_id,
            ))?;
            state.op_state = result.new_state;
            op_id
        };
        self.save_state()?;
        Ok(op_id)
    }

    /// Finish an allocation operation (test helper).
    ///
    /// Transitions the kernel from Allocating back to Idle.
    #[cfg(any(test, feature = "testutils"))]
    pub fn finish_allocating(
        &mut self,
        caller: Address,
        op_id: u64,
    ) -> Result<AllocationResult, RuntimeError> {
        self.authorize(ActionKind::FinishAllocating, caller)?;
        let result = {
            let state = self.state_mut()?;
            let transition_result = transition_to_runtime(complete_allocation(
                mem::take(&mut state.op_state),
                op_id,
                None,
            ))?;
            state.op_state = transition_result.new_state;
            AllocationResult {
                op_id,
                new_external_assets: state.external_assets,
                summary: EffectSummary::new(),
            }
        };
        self.save_state()?;
        Ok(result)
    }

    /// Begin a refresh operation.
    ///
    /// Filters the plan to exclude locked markets before starting.
    ///
    pub fn begin_refreshing(
        &mut self,
        caller: Address,
        plan: Vec<TargetId>,
        current_ns: u64,
    ) -> Result<u64, RuntimeError> {
        // Filter plan to exclude locked markets
        let filtered_plan =
            build_refresh_plan_with_locks(&plan, &self.policy_state.locks, current_ns);

        if filtered_plan.is_empty() {
            return Err(RuntimeError::invalid_input("empty refresh plan"));
        }

        self.authorize(ActionKind::BeginRefreshing, caller)?;
        let op_id = {
            let state = self.state_mut()?;
            let op_id = Self::reserve_op_id(state)?;
            let transition_result = transition_to_runtime(start_refresh(
                mem::take(&mut state.op_state),
                filtered_plan,
                op_id,
            ))?;
            state.op_state = transition_result.new_state;
            op_id
        };
        self.save_state()?;
        Ok(op_id)
    }

    /// Finish a refresh operation.
    ///
    pub fn finish_refreshing(
        &mut self,
        caller: Address,
        op_id: u64,
    ) -> Result<RefreshResult, RuntimeError> {
        self.authorize(ActionKind::FinishRefreshing, caller)?;
        let result = {
            let state = self.state_mut()?;
            let markets_refreshed = match &state.op_state {
                OpState::Refreshing(refresh) => refresh.plan.len() as u32,
                _ => 0,
            };
            let transition_result =
                transition_to_runtime(complete_refresh(mem::take(&mut state.op_state), op_id))?;
            state.op_state = transition_result.new_state;

            RefreshResult {
                op_id,
                markets_refreshed,
                new_external_assets: state.external_assets,
            }
        };
        self.save_state()?;
        Ok(result)
    }

    /// Get a reference to the current policy state.
    #[inline]
    #[must_use]
    pub fn policy_state(&self) -> &PolicyState {
        &self.policy_state
    }

    /// Get the current kernel restrictions.
    #[inline]
    #[must_use]
    pub fn restrictions(&self) -> Option<&Restrictions> {
        self.restrictions.as_ref()
    }

    /// Get a mutable reference to the current policy state.
    #[inline]
    pub fn policy_state_mut(&mut self) -> &mut PolicyState {
        &mut self.policy_state
    }

    pub fn get_fee_anchor(&self) -> Result<FeeAccrualAnchor, RuntimeError> {
        Ok(self.state()?.fee_anchor.clone())
    }

    pub fn get_fees(&self) -> &FeesSpec {
        &self.config.fees
    }

    pub fn get_cap_groups(&self) -> Vec<(CapGroupId, CapGroupRecord)> {
        self.policy_state
            .cap_groups
            .iter()
            .map(|(id, rec)| (id.clone(), rec.clone()))
            .collect()
    }

    pub fn queue_tail(&self) -> Result<u64, RuntimeError> {
        Ok(self.state()?.withdraw_queue.next_pending_withdrawal_id)
    }

    pub fn peek_next_pending_withdrawal_id(&self) -> Result<Option<u64>, RuntimeError> {
        Ok(self.state()?.withdraw_queue.head().map(|(id, _)| id))
    }

    pub fn get_withdrawing_op_id(&self) -> Result<Option<u64>, RuntimeError> {
        let state = self.state()?;
        match &state.op_state {
            OpState::Withdrawing(w) => Ok(Some(w.op_id)),
            _ => Ok(None),
        }
    }

    pub fn get_current_withdraw_request_id(&self) -> Result<Option<u64>, RuntimeError> {
        let state = self.state()?;
        match &state.op_state {
            OpState::Withdrawing(_) | OpState::Payout(_) => {
                Ok(Some(state.withdraw_queue.next_withdraw_to_execute))
            }
            _ => Ok(None),
        }
    }

    pub fn set_supply_queue(
        &mut self,
        caller: Address,
        target_ids: Vec<TargetId>,
    ) -> Result<(), RuntimeError> {
        self.auth
            .authorize(ActionKind::SetRestrictions, caller, None)?;

        let mut entries = Vec::with_capacity(target_ids.len());
        for target_id in target_ids {
            if entries
                .iter()
                .any(|entry: &SupplyQueueEntry| entry.target_id == target_id)
            {
                return Err(RuntimeError::invalid_input(
                    "duplicate market in supply queue",
                ));
            }
            entries.push(SupplyQueueEntry::new(target_id, 1));
        }

        let mut next_policy = self.policy_state.clone();
        next_policy.supply_queue = SupplyQueue::from(entries);
        self.storage.save_policy_state(&next_policy)?;
        self.policy_state = next_policy;

        Ok(())
    }

    pub fn set_cap(
        &mut self,
        caller: Address,
        market_id: TargetId,
        new_cap: u128,
    ) -> Result<(), RuntimeError> {
        self.auth
            .authorize(ActionKind::SetRestrictions, caller, None)?;

        let current_cap = self.policy_state.markets.get(&market_id).map(|m| m.cap);
        let decision = cap_change_decision(current_cap, new_cap)
            .map_err(|_| RuntimeError::invalid_input("cap unchanged"))?;
        if matches!(decision, TimelockDecision::Timelocked) {
            return Err(RuntimeError::invalid_input(
                "cap increase or new market requires timelock",
            ));
        }

        let Some(config): Option<&mut MarketConfig> = self.policy_state.markets.get_mut(&market_id)
        else {
            return Err(RuntimeError::invalid_input("market not found"));
        };
        config.cap = new_cap;

        self.storage.save_policy_state(&self.policy_state)?;
        Ok(())
    }

    pub fn remove_market(
        &mut self,
        caller: Address,
        market_id: TargetId,
    ) -> Result<(), RuntimeError> {
        self.auth
            .authorize(ActionKind::SetRestrictions, caller, None)?;

        let principal = self.policy_state.principal_for(market_id);

        let Some(config) = self.policy_state.markets.get_mut(&market_id) else {
            return Err(RuntimeError::invalid_input("market not found"));
        };
        if config.cap > 0 {
            return Err(RuntimeError::invalid_input(
                "cannot remove market with non-zero cap",
            ));
        }
        if !config.enabled {
            return Err(RuntimeError::invalid_input("market already removed"));
        }

        if market_removal_decision(principal).requires_timelock() {
            return Err(RuntimeError::invalid_input(
                "market with principal requires timelock",
            ));
        }

        config.enabled = false;
        self.storage.save_policy_state(&self.policy_state)?;
        Ok(())
    }

    #[inline(never)]
    pub fn update_cap_group(
        &mut self,
        caller: Address,
        update: CapGroupUpdate,
    ) -> Result<(), RuntimeError> {
        self.auth
            .authorize(ActionKind::SetRestrictions, caller, None)?;

        match update {
            CapGroupUpdate::SetCap {
                cap_group_id,
                new_cap,
            } => {
                let current = self
                    .policy_state
                    .cap_groups
                    .get(&cap_group_id)
                    .and_then(|record| record.cap.absolute_cap.map(NonZeroU128::get));
                let decision = cap_change_decision(current, new_cap)
                    .map_err(|_| RuntimeError::invalid_input("cap group cap unchanged"))?;
                if matches!(decision, TimelockDecision::Timelocked) {
                    return Err(RuntimeError::invalid_input(
                        "cap group cap increase requires timelock",
                    ));
                }

                let record = self
                    .policy_state
                    .cap_groups
                    .get_mut(&cap_group_id)
                    .ok_or_else(|| RuntimeError::invalid_input("cap group not found"))?;
                record.cap.absolute_cap = NonZeroU128::new(new_cap);
            }
            CapGroupUpdate::SetRelativeCap {
                cap_group_id,
                new_relative_cap_wad,
            } => {
                let proposed = Wad::from(new_relative_cap_wad);
                let current = self
                    .policy_state
                    .cap_groups
                    .get(&cap_group_id)
                    .and_then(|record| record.cap.relative_cap);
                let decision = relative_cap_change_decision(current, proposed).map_err(|_| {
                    RuntimeError::invalid_input("invalid cap group relative cap change")
                })?;
                if matches!(decision, TimelockDecision::Timelocked) {
                    return Err(RuntimeError::invalid_input(
                        "cap group relative cap increase requires timelock",
                    ));
                }

                let record = self
                    .policy_state
                    .cap_groups
                    .get_mut(&cap_group_id)
                    .ok_or_else(|| RuntimeError::invalid_input("cap group not found"))?;
                record.cap.relative_cap = if proposed.is_zero() {
                    None
                } else {
                    Some(proposed)
                };
            }
            CapGroupUpdate::SetMembership {
                market_id,
                cap_group_id,
            } => {
                let changed = {
                    let market = self
                        .policy_state
                        .markets
                        .get(&market_id)
                        .ok_or_else(|| RuntimeError::invalid_input("market not found"))?;
                    market.cap_group_id != cap_group_id
                };
                let _decision = membership_change_decision(changed)
                    .map_err(|_| RuntimeError::invalid_input("membership unchanged"))?;

                if let Some(group_id) = cap_group_id.as_ref() {
                    if !self.policy_state.cap_groups.contains_key(group_id) {
                        self.policy_state
                            .cap_groups
                            .insert(group_id.clone(), CapGroupRecord::default());
                    }
                }

                let market = self
                    .policy_state
                    .markets
                    .get_mut(&market_id)
                    .ok_or_else(|| RuntimeError::invalid_input("market not found"))?;

                market.cap_group_id = cap_group_id;
                self.policy_state.refresh_cap_group_principals();
            }
        }

        self.storage.save_policy_state(&self.policy_state)?;
        Ok(())
    }

    pub fn supply_queue_targets(&self) -> Vec<TargetId> {
        self.policy_state
            .supply_queue
            .entries
            .iter()
            .map(|entry| entry.target_id)
            .collect()
    }
}

#[contract]
pub struct SorobanVaultContract;

type ContractVault<'a> = CuratorVault<
    SorobanStorage<'a>,
    RbacAuth,
    SorobanEffectInterpreter<'a, ShareTokenAdapter<'a>, SdkTokenAdapter<'a>>,
>;

/// Emit a lightweight admin/governance event.
/// Uses raw Soroban events with `symbol_short!` topics to avoid postcard overhead.
#[allow(deprecated)] // intentionally avoiding #[contractevent] to reduce WASM spec size
#[inline(never)]
fn emit_admin_event(env: &Env, action: soroban_sdk::Symbol) {
    env.events().publish((symbol_short!("admin"),), action);
}

/// Emit an allocation event with market and amount data.
#[allow(deprecated)] // intentionally avoiding #[contractevent] to reduce WASM spec size
#[inline(never)]
fn emit_alloc_event(env: &Env, market: u32, amount: i128, supply: bool) {
    env.events()
        .publish((symbol_short!("alloc"), supply), (market, amount));
}

/// Check that the adapter is in the governance-managed allowlist.
/// If no allowlist is set, all adapters are allowed (backwards-compatible).
fn require_allowed_adapter(env: &Env, adapter: &SdkAddress) -> Result<(), ContractError> {
    let allowed: Option<soroban_sdk::Vec<SdkAddress>> =
        env.storage().instance().get(&VaultDataKey::AllowedAdapters);
    if let Some(list) = allowed {
        for a in list.iter() {
            if a == *adapter {
                return Ok(());
            }
        }
        return Err(ContractError::Unauthorized);
    }
    Ok(())
}

fn adapter_for_market(env: &Env, market: u32) -> Result<SdkAddress, ContractError> {
    let adapters: Option<soroban_sdk::Vec<SdkAddress>> =
        env.storage().instance().get(&VaultDataKey::AllowedAdapters);
    let Some(adapters) = adapters else {
        return Err(ContractError::InvalidInput);
    };

    let mut queue_index: Option<u32> = None;
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        for (idx, target_id) in vault.supply_queue_targets().iter().enumerate() {
            if *target_id == market {
                queue_index = Some(
                    u32::try_from(idx)
                        .map_err(|_| RuntimeError::invalid_input("index overflow"))?,
                );
                return Ok(());
            }
        }
        Err(RuntimeError::invalid_input("market not in supply queue"))
    };
    with_contract_vault_contract_error(env, &mut call)?;

    let index = queue_index.ok_or(ContractError::InvalidInput)?;
    adapters.get(index).ok_or(ContractError::InvalidInput)
}

fn current_supply_queue_len(env: &Env) -> Result<u32, ContractError> {
    let mut len: u32 = 0;
    let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
        len = u32::try_from(vault.supply_queue_targets().len())
            .map_err(|_| RuntimeError::invalid_input("queue overflow"))?;
        Ok(())
    };
    with_contract_vault_contract_error(env, &mut call)?;
    Ok(len)
}

fn apply_fee_change(
    env: &Env,
    performance_fee_wad: i128,
    performance_recipient: SdkAddress,
    management_fee_wad: i128,
    management_recipient: SdkAddress,
    max_growth_rate_wad: Option<i128>,
) -> Result<(), ContractError> {
    if performance_fee_wad < 0 || management_fee_wad < 0 {
        return Err(ContractError::InvalidInput);
    }
    if performance_fee_wad as u128 > MAX_PERFORMANCE_FEE_WAD {
        return Err(ContractError::InvalidInput);
    }
    if management_fee_wad as u128 > MAX_MANAGEMENT_FEE_WAD {
        return Err(ContractError::InvalidInput);
    }

    let max_rate = match max_growth_rate_wad {
        Some(value) => {
            if value < 0 {
                return Err(ContractError::InvalidInput);
            }
            Some(Wad::from(value as u128))
        }
        None => None,
    };

    let performance_kernel = kernel_address_from_sdk(env, &performance_recipient);
    let management_kernel = kernel_address_from_sdk(env, &management_recipient);
    let fees = FeesSpec::new(
        FeeSlot::new(Wad::from(performance_fee_wad as u128), performance_kernel),
        FeeSlot::new(Wad::from(management_fee_wad as u128), management_kernel),
        max_rate,
    );

    runtime_to_contract(store_fees_spec(env, &fees))?;
    let storage = SorobanStorage::new(env);
    storage.save_address(&performance_kernel, &performance_recipient);
    storage.save_address(&management_kernel, &management_recipient);
    Ok(())
}

fn extend_storage_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
    let storage = SorobanStorage::new(env);
    storage.extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
}

// Read a required `SdkAddress` from instance storage.
pub(crate) fn get_config_address(
    env: &Env,
    key: &soroban_sdk::Symbol,
) -> Result<SdkAddress, ContractError> {
    match env.storage().instance().get(key) {
        Some(address) => Ok(address),
        None => Err(ContractError::MissingConfig),
    }
}

#[inline]
fn require_config_address(
    env: &Env,
    key: &soroban_sdk::Symbol,
    msg: &'static str,
) -> Result<SdkAddress, RuntimeError> {
    match get_config_address(env, key) {
        Ok(addr) => Ok(addr),
        Err(_) => Err(RuntimeError::storage_error(msg)),
    }
}

/// Write an `SdkAddress` into instance storage.
pub(crate) fn set_config_address(env: &Env, key: &soroban_sdk::Symbol, addr: &SdkAddress) {
    env.storage().instance().set(key, addr);
}

fn query_vault_field(env: &Env, f: fn(&VaultState) -> u128) -> i128 {
    let storage = SorobanStorage::new(env);
    match storage.load_state() {
        Ok(Some(versioned)) => to_i128(f(&versioned.state)).unwrap_or(0),
        Ok(None) => 0,
        Err(_) => 0,
    }
}

fn query_vault_snapshot(env: &Env) -> (i128, i128, i128) {
    let storage = SorobanStorage::new(env);
    match storage.load_state() {
        Ok(Some(versioned)) => (
            to_i128(versioned.state.total_shares).unwrap_or(0),
            to_i128(versioned.state.idle_assets).unwrap_or(0),
            to_i128(versioned.state.external_assets).unwrap_or(0),
        ),
        Ok(None) | Err(_) => (0, 0, 0),
    }
}

fn sdk_string_to_alloc(value: soroban_sdk::String) -> Result<AllocString, ContractError> {
    let bytes = value.to_bytes();
    let mut raw = vec![0u8; bytes.len() as usize];
    bytes.copy_into_slice(&mut raw);
    match AllocString::from_utf8(raw) {
        Ok(s) => Ok(s),
        Err(_) => Err(ContractError::InvalidInput),
    }
}

/// Migrate pause state from legacy storage locations to OZ Pausable storage.
///
/// Handles migration from:
/// 1. `VaultDataKey::Paused` (oldest location)
/// 2. `SorobanStorageKey::Paused` (intermediate location)
///
/// Both are migrated to OZ's `PausableStorageKey::Paused`.
fn migrate_legacy_paused(env: &Env) {
    let storage = SorobanStorage::new(env);

    // Migrate from VaultDataKey::Paused (oldest)
    if let Some(paused) = env
        .storage()
        .instance()
        .get::<_, bool>(&VaultDataKey::Paused)
    {
        storage.set_paused(paused);
        env.storage().instance().remove(&VaultDataKey::Paused);
        return; // Only one legacy location should exist
    }

    // Migrate from SorobanStorageKey::Paused (intermediate)
    if let Some(paused) = storage.take_legacy_paused() {
        storage.set_paused(paused);
    }
}

#[inline(never)]
fn load_vault_bootstrap<'a>(env: &'a Env) -> Result<VaultBootstrap<'a>, RuntimeError> {
    // Block operations during migration (upgrade in progress)
    if stellar_contract_utils::upgradeable::can_complete_migration(env) {
        return Err(RuntimeError::invalid_state(
            "migration in progress - call migrate() first",
        ));
    }

    extend_storage_ttl(env);
    migrate_legacy_paused(env);
    let curator: SdkAddress =
        require_config_address(env, &VaultDataKey::Curator, "curator not set")?;
    let governance: SdkAddress =
        require_config_address(env, &VaultDataKey::Governance, "governance not set")?;
    let asset_token: SdkAddress =
        require_config_address(env, &VaultDataKey::AssetToken, "asset token not set")?;
    let share_token: SdkAddress =
        require_config_address(env, &VaultDataKey::ShareToken, "share token not set")?;

    let vault_sdk = env.current_contract_address();
    let vault_kernel = kernel_address_from_sdk(env, &vault_sdk);
    let curator_kernel = kernel_address_from_sdk(env, &curator);
    let governance_kernel = kernel_address_from_sdk(env, &governance);
    let asset_kernel = kernel_address_from_sdk(env, &asset_token);
    let share_kernel = kernel_address_from_sdk(env, &share_token);

    let mut config = ContractConfig::new(
        curator_kernel,
        vault_kernel,
        Vec::new(),
        Vec::new(),
        asset_kernel,
        share_kernel,
    );

    let fees = load_fees_spec(env)?;
    config = config.with_fees(fees);

    let storage = SorobanStorage::new(env);
    let paused = storage.is_paused();
    let mut rbac_config = RbacConfig::with_curator(curator_kernel);
    rbac_config.add_role(governance_kernel, Role::Curator);
    // Load guardians from storage and add to RBAC config.
    let guard_addrs: Option<soroban_sdk::Vec<SdkAddress>> =
        env.storage().instance().get(&VaultDataKey::Guardians);
    if let Some(guardians) = guard_addrs {
        for g in guardians.iter() {
            rbac_config.add_role(kernel_address_from_sdk(env, &g), Role::Guardian);
        }
    }

    // Load allocators from storage and add to RBAC config.
    let alloc_addrs: Option<soroban_sdk::Vec<SdkAddress>> =
        env.storage().instance().get(&VaultDataKey::Allocators);
    if let Some(allocators) = alloc_addrs {
        for a in allocators.iter() {
            rbac_config.add_role(kernel_address_from_sdk(env, &a), Role::Allocator);
        }
    }
    let sentinel: Option<SdkAddress> = env.storage().instance().get(&VaultDataKey::Sentinel);
    if let Some(sentinel_addr) = sentinel {
        rbac_config.add_role(kernel_address_from_sdk(env, &sentinel_addr), Role::Sentinel);
    }
    rbac_config.set_paused(paused);
    let auth = RbacAuth::new(rbac_config);

    Ok(VaultBootstrap {
        config,
        storage,
        auth,
        asset_token,
        share_token,
    })
}

type ContractVaultCallback<'a> =
    dyn for<'b> FnMut(&mut ContractVault<'b>) -> Result<(), RuntimeError> + 'a;

#[inline(never)]
fn with_contract_vault(env: &Env, f: &mut ContractVaultCallback<'_>) -> Result<(), RuntimeError> {
    let bootstrap = load_vault_bootstrap(env)?;
    let share_adapter = ShareTokenAdapter::new(env, &bootstrap.share_token);
    let asset_adapter = SdkTokenAdapter::new(env, &bootstrap.asset_token);
    let interpreter = SorobanEffectInterpreter::new(env, &share_adapter, &asset_adapter);

    let mut vault = CuratorVault::new(
        bootstrap.config,
        bootstrap.storage,
        bootstrap.auth,
        interpreter,
    );
    vault.load_state()?;
    f(&mut vault)
}

#[inline]
fn kernel_to_runtime<T>(result: Result<T, KernelError>) -> Result<T, RuntimeError> {
    match result {
        Ok(value) => Ok(value),
        Err(_) => Err(RuntimeError::transition_error()),
    }
}

#[inline]
fn transition_to_runtime<T>(result: Result<T, TransitionError>) -> Result<T, RuntimeError> {
    match result {
        Ok(value) => Ok(value),
        Err(_) => Err(RuntimeError::transition_error()),
    }
}

#[inline]
fn with_contract_vault_contract_error(
    env: &Env,
    f: &mut ContractVaultCallback<'_>,
) -> Result<(), ContractError> {
    runtime_to_contract(with_contract_vault(env, f))
}

fn with_reentrancy_guard(
    env: &Env,
    f: &mut dyn FnMut() -> Result<(), ContractError>,
) -> Result<(), ContractError> {
    ensure_not_reentrant(env)?;
    env.storage()
        .instance()
        .set(&VaultDataKey::ReentrancyLock, &true);
    let result = f();
    env.storage()
        .instance()
        .set(&VaultDataKey::ReentrancyLock, &false);
    result
}

#[inline(never)]
fn with_reentrancy_guarded_contract_vault(
    env: &Env,
    f: &mut ContractVaultCallback<'_>,
) -> Result<(), ContractError> {
    with_reentrancy_guard(env, &mut || with_contract_vault_contract_error(env, f))
}

fn ensure_not_reentrant(env: &Env) -> Result<(), ContractError> {
    let locked: bool = env
        .storage()
        .instance()
        .get(&VaultDataKey::ReentrancyLock)
        .unwrap_or(false);
    if locked {
        Err(ContractError::Reentrancy)
    } else {
        Ok(())
    }
}

#[contractimpl]
impl SorobanVaultContract {
    pub fn initialize(
        env: Env,
        curator: soroban_sdk::Address,
        governance: soroban_sdk::Address,
        asset_token: soroban_sdk::Address,
        share_token: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        // Check not already initialized
        if env.storage().instance().has(&VaultDataKey::Initialized) {
            return Err(ContractError::AlreadyInitialized);
        }

        // Store configuration
        set_config_address(&env, &VaultDataKey::Curator, &curator);
        set_config_address(&env, &VaultDataKey::Governance, &governance);
        set_config_address(&env, &VaultDataKey::AssetToken, &asset_token);
        set_config_address(&env, &VaultDataKey::ShareToken, &share_token);
        set_config_address(&env, &VaultDataKey::SkimRecipient, &governance);
        env.storage()
            .instance()
            .set(&VaultDataKey::ReentrancyLock, &false);
        env.storage()
            .instance()
            .set(&VaultDataKey::Initialized, &true);
        runtime_to_contract(store_fees_spec(&env, &FeesSpec::zero()))?;

        // Initialize vault state in persistent storage using current version.
        let mut storage = SorobanStorage::new(&env);
        let versioned = VersionedState::new(VaultState::default());
        runtime_to_contract(storage.save_state(&versioned))?;
        runtime_to_contract(storage.save_paused(false))?;
        Ok(())
    }

    pub fn deposit_with_min(
        env: Env,
        owner: soroban_sdk::Address,
        receiver: soroban_sdk::Address,
        assets: i128,
        min_shares_out: i128,
    ) -> Result<i128, ContractError> {
        // Require authorization from owner
        require_signed(&owner);

        if assets <= 0 {
            return Err(ContractError::InvalidInput);
        }

        let assets_u128 = to_u128(assets)?;
        let min_shares_u128 = if min_shares_out < 0 {
            return Err(ContractError::InvalidInput);
        } else {
            to_u128(min_shares_out)?
        };
        let now_ns = ledger_timestamp_ns(&env)?;

        let mut shares_minted = 0u128;
        with_reentrancy_guard(&env, &mut || {
            let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
                let result = vault.deposit_soroban(
                    &env,
                    owner.clone(),
                    receiver.clone(),
                    assets_u128,
                    min_shares_u128,
                    now_ns,
                )?;
                shares_minted = result.shares_minted;
                Ok(())
            };
            with_contract_vault_contract_error(&env, &mut call)
        })?;
        to_i128(shares_minted)
    }

    pub fn request_withdraw(
        env: Env,
        owner: soroban_sdk::Address,
        receiver: soroban_sdk::Address,
        shares: i128,
        min_assets_out: i128,
    ) -> Result<u64, ContractError> {
        // Require authorization from owner
        require_signed(&owner);

        if shares <= 0 {
            return Err(ContractError::InvalidInput);
        }
        let shares_u128 = to_u128(shares)?;
        let min_assets_u128 = if min_assets_out < 0 {
            return Err(ContractError::InvalidInput);
        } else {
            to_u128(min_assets_out)?
        };
        let now_ns = ledger_timestamp_ns(&env)?;

        let mut request_id = 0u64;
        with_reentrancy_guard(&env, &mut || {
            let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
                let result = vault.request_withdraw_soroban(
                    &env,
                    owner.clone(),
                    receiver.clone(),
                    shares_u128,
                    min_assets_u128,
                    now_ns,
                )?;
                request_id = result.request_id;
                Ok(())
            };
            with_contract_vault_contract_error(&env, &mut call)
        })?;
        Ok(request_id)
    }

    pub fn execute_withdraw(env: Env, caller: soroban_sdk::Address) -> Result<(), ContractError> {
        require_signed(&caller);
        let now_ns = ledger_timestamp_ns(&env)?;

        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault
                .execute_withdraw_soroban(&env, caller.clone(), now_ns)
                .map(|_| ())
        };
        with_reentrancy_guarded_contract_vault(&env, &mut call)
    }

    pub fn allocate_supply(
        env: Env,
        caller: soroban_sdk::Address,
        market: u32,
        amount: i128,
    ) -> Result<i128, ContractError> {
        require_signed(&caller);
        let adapter = adapter_for_market(&env, market)?;
        require_allowed_adapter(&env, &adapter)?;
        if amount <= 0 {
            return Err(ContractError::InvalidInput);
        }
        let amount_u128 = to_u128(amount)?;
        let mut new_external: u128 = 0;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let caller_kernel = kernel_address_from_sdk(&env, &caller);
            let plan = vec![(market.into(), amount_u128)];
            let op_id = vault.begin_allocation_internal(caller_kernel, &plan)?;
            {
                let state = vault.state_mut()?;
                state.external_assets = state.external_assets.saturating_add(amount_u128);
                state.sync_total_assets();
            }
            vault.finish_allocation_internal(op_id)?;
            vault.save_state()?;
            new_external = vault.state()?.external_assets;
            Ok(())
        };
        with_reentrancy_guarded_contract_vault(&env, &mut call)?;

        emit_alloc_event(&env, market, amount, true);
        // transfer tokens from vault to adapter, then invoke adapter.supply()
        let asset_token = get_config_address(&env, &VaultDataKey::AssetToken)?;
        soroban_sdk::token::Client::new(&env, &asset_token).transfer(
            &env.current_contract_address(),
            &adapter,
            &amount,
        );
        call_adapter(&env, &adapter, "supply", &asset_token, amount);

        to_i128(new_external)
    }

    pub fn allocate_withdraw(
        env: Env,
        caller: soroban_sdk::Address,
        #[allow(unused_variables)] market: u32,
        amount: i128,
    ) -> Result<i128, ContractError> {
        require_signed(&caller);
        let adapter = adapter_for_market(&env, market)?;
        require_allowed_adapter(&env, &adapter)?;
        if amount <= 0 {
            return Err(ContractError::InvalidInput);
        }
        let amount_u128 = to_u128(amount)?;

        // invoke adapter.withdraw() first — sends tokens to vault
        let asset_token = get_config_address(&env, &VaultDataKey::AssetToken)?;
        call_adapter(&env, &adapter, "withdraw", &asset_token, amount);

        // then update accounting (external → idle)
        let mut new_external: u128 = 0;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let caller_kernel = kernel_address_from_sdk(&env, &caller);
            vault.authorize(ActionKind::BeginAllocating, caller_kernel)?;
            {
                let state = vault.state_mut()?;
                state.external_assets = state.external_assets.saturating_sub(amount_u128);
                state.idle_assets = state.idle_assets.saturating_add(amount_u128);
                state.sync_total_assets();
                new_external = state.external_assets;
            }
            vault.save_state()?;
            Ok(())
        };
        with_reentrancy_guarded_contract_vault(&env, &mut call)?;
        emit_alloc_event(&env, market, amount, false);
        to_i128(new_external)
    }

    pub fn refresh_markets(
        env: Env,
        caller: soroban_sdk::Address,
        markets: soroban_sdk::Vec<u32>,
    ) -> Result<i128, ContractError> {
        require_signed(&caller);
        let now_ns = ledger_timestamp_ns(&env)?;

        let markets_vec: Vec<TargetId> = markets.iter().collect();

        let mut new_external: u128 = 0;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let caller_kernel = kernel_address_from_sdk(&env, &caller);
            let op_id = vault.begin_refreshing(caller_kernel, markets_vec.clone(), now_ns)?;
            let result = vault.finish_refreshing(caller_kernel, op_id)?;
            new_external = result.new_external_assets;
            Ok(())
        };
        with_reentrancy_guarded_contract_vault(&env, &mut call)?;
        to_i128(new_external)
    }

    pub fn set_paused(
        env: Env,
        caller: soroban_sdk::Address,
        paused: bool,
    ) -> Result<(), ContractError> {
        use stellar_contract_utils::pausable::{emit_paused, emit_unpaused};

        ensure_not_reentrant(&env)?;
        let caller_kernel = governance_caller(&env, &caller)?;

        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.pause(caller_kernel, paused)
        };
        with_contract_vault_contract_error(&env, &mut call)?;

        // Emit OZ Pausable events
        if paused {
            emit_paused(&env);
        } else {
            emit_unpaused(&env);
        }

        // Emit kernel event
        crate::effects::publish_kernel_event(
            &env,
            &templar_vault_kernel::effects::KernelEvent::PauseUpdated { paused },
        );
        Ok(())
    }

    pub fn set_curator(
        env: Env,
        caller: soroban_sdk::Address,
        new_curator: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;
        set_config_address(&env, &VaultDataKey::Curator, &new_curator);
        emit_admin_event(&env, symbol_short!("s_curatr"));
        Ok(())
    }

    pub fn set_governance(
        env: Env,
        caller: soroban_sdk::Address,
        governance: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;
        require_contract_address(&governance)?;
        set_config_address(&env, &VaultDataKey::Governance, &governance);
        emit_admin_event(&env, symbol_short!("s_gov"));
        Ok(())
    }

    pub fn set_share_token(
        env: Env,
        caller: soroban_sdk::Address,
        _share_token: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;
        Err(ContractError::InvalidState)
    }

    pub fn set_supply_queue(
        env: Env,
        caller: soroban_sdk::Address,
        target_ids: soroban_sdk::Vec<u32>,
    ) -> Result<(), ContractError> {
        if let Some(adapters) = env
            .storage()
            .instance()
            .get::<_, soroban_sdk::Vec<SdkAddress>>(&VaultDataKey::AllowedAdapters)
        {
            if adapters.len() != target_ids.len() {
                return Err(ContractError::InvalidInput);
            }
        }
        let caller_kernel = governance_caller(&env, &caller)?;
        let mut queue_targets = Vec::with_capacity(target_ids.len() as usize);
        for target_id in target_ids.iter() {
            queue_targets.push(target_id);
        }

        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.set_supply_queue(caller_kernel, queue_targets.clone())?;
            Ok(())
        };
        with_reentrancy_guarded_contract_vault(&env, &mut call)
    }

    pub fn set_cap(
        env: Env,
        caller: soroban_sdk::Address,
        market_id: u32,
        new_cap: i128,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;
        let new_cap_u128 = to_u128(new_cap)?;

        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.set_cap(caller_kernel, market_id, new_cap_u128)
        };
        with_reentrancy_guarded_contract_vault(&env, &mut call)
    }

    pub fn remove_market(
        env: Env,
        caller: soroban_sdk::Address,
        market_id: u32,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;

        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.remove_market(caller_kernel, market_id)
        };
        with_reentrancy_guarded_contract_vault(&env, &mut call)
    }

    pub fn set_group_cap(
        env: Env,
        caller: soroban_sdk::Address,
        cap_group_id: soroban_sdk::String,
        new_cap: i128,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;
        let internal = CapGroupUpdate::SetCap {
            cap_group_id: CapGroupId::new(sdk_string_to_alloc(cap_group_id)?),
            new_cap: to_u128(new_cap)?,
        };
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.update_cap_group(caller_kernel, internal.clone())
        };
        with_reentrancy_guarded_contract_vault(&env, &mut call)
    }

    pub fn set_group_rel_cap(
        env: Env,
        caller: soroban_sdk::Address,
        cap_group_id: soroban_sdk::String,
        new_relative_cap_wad: i128,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;
        let internal = CapGroupUpdate::SetRelativeCap {
            cap_group_id: CapGroupId::new(sdk_string_to_alloc(cap_group_id)?),
            new_relative_cap_wad: to_u128(new_relative_cap_wad)?,
        };
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.update_cap_group(caller_kernel, internal.clone())
        };
        with_reentrancy_guarded_contract_vault(&env, &mut call)
    }

    pub fn set_group_member(
        env: Env,
        caller: soroban_sdk::Address,
        market_id: u32,
        cap_group_id: soroban_sdk::String,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;
        let s = sdk_string_to_alloc(cap_group_id)?;
        let group = if s.is_empty() {
            None
        } else {
            Some(CapGroupId::new(s))
        };
        let internal = CapGroupUpdate::SetMembership {
            market_id,
            cap_group_id: group,
        };
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.update_cap_group(caller_kernel, internal.clone())
        };
        with_reentrancy_guarded_contract_vault(&env, &mut call)
    }

    pub fn set_fees(
        env: Env,
        caller: soroban_sdk::Address,
        performance_fee_wad: i128,
        performance_recipient: soroban_sdk::Address,
        management_fee_wad: i128,
        management_recipient: soroban_sdk::Address,
        max_growth_rate_wad: Option<i128>,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;

        apply_fee_change(
            &env,
            performance_fee_wad,
            performance_recipient,
            management_fee_wad,
            management_recipient,
            max_growth_rate_wad,
        )?;
        emit_admin_event(&env, symbol_short!("s_fees"));
        Ok(())
    }

    pub fn accept_fees(env: Env, caller: soroban_sdk::Address) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;
        Err(ContractError::InvalidState)
    }

    pub fn revoke_pending_fees(
        env: Env,
        caller: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;
        Err(ContractError::InvalidState)
    }

    pub fn pending_fees_valid_at(env: Env) -> Result<Option<u64>, ContractError> {
        ensure_not_reentrant(&env)?;
        Ok(None)
    }

    pub fn set_restrictions(
        env: Env,
        caller: soroban_sdk::Address,
        mode: u32,
        accounts: soroban_sdk::Vec<soroban_sdk::Address>,
    ) -> Result<(), ContractError> {
        let caller_kernel = governance_caller(&env, &caller)?;

        let mut kernel_accounts = Vec::with_capacity(accounts.len() as usize);
        for account in accounts.iter() {
            kernel_accounts.push(kernel_address_from_sdk(&env, &account));
        }

        let restrictions = match mode {
            0 => None,
            1 => Some(Restrictions::Paused),
            2 => Some(Restrictions::Blacklist(kernel_accounts)),
            3 => Some(Restrictions::Whitelist(kernel_accounts)),
            _ => return Err(ContractError::InvalidInput),
        };

        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            vault.set_restrictions(caller_kernel, restrictions.clone())?;
            Ok(())
        };
        with_reentrancy_guarded_contract_vault(&env, &mut call)?;
        emit_admin_event(&env, symbol_short!("s_rstrct"));
        Ok(())
    }

    /// Set the guardian addresses. Guardians can pause/unpause the vault.
    pub fn set_sentinel(
        env: Env,
        caller: soroban_sdk::Address,
        sentinel: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;
        env.storage()
            .instance()
            .set(&VaultDataKey::Sentinel, &sentinel);
        emit_admin_event(&env, symbol_short!("s_sntnl"));
        Ok(())
    }

    /// Set the guardian addresses. Guardians can pause/unpause the vault.
    pub fn set_guardians(
        env: Env,
        caller: soroban_sdk::Address,
        guardians: soroban_sdk::Vec<soroban_sdk::Address>,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;
        env.storage()
            .instance()
            .set(&VaultDataKey::Guardians, &guardians);
        emit_admin_event(&env, symbol_short!("s_guards"));
        Ok(())
    }

    /// Set the allocator addresses. Allocators can manage allocations and refreshes.
    pub fn set_allocators(
        env: Env,
        caller: soroban_sdk::Address,
        allocators: soroban_sdk::Vec<soroban_sdk::Address>,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;
        env.storage()
            .instance()
            .set(&VaultDataKey::Allocators, &allocators);
        emit_admin_event(&env, symbol_short!("s_allctr"));
        Ok(())
    }

    pub fn set_allowed_adapters(
        env: Env,
        caller: soroban_sdk::Address,
        adapters: soroban_sdk::Vec<soroban_sdk::Address>,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;
        let queue_len = current_supply_queue_len(&env)?;
        if queue_len > 0 && adapters.len() != queue_len {
            return Err(ContractError::InvalidInput);
        }
        if adapters.is_empty() {
            env.storage()
                .instance()
                .remove(&VaultDataKey::AllowedAdapters);
        } else {
            env.storage()
                .instance()
                .set(&VaultDataKey::AllowedAdapters, &adapters);
        }
        emit_admin_event(&env, symbol_short!("s_adaptr"));
        Ok(())
    }

    pub fn set_skim_recipient(
        env: Env,
        caller: soroban_sdk::Address,
        recipient: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;
        set_config_address(&env, &VaultDataKey::SkimRecipient, &recipient);
        emit_admin_event(&env, symbol_short!("s_skimr"));
        Ok(())
    }

    pub fn skim(
        env: Env,
        caller: soroban_sdk::Address,
        token: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;
        let asset = get_config_address(&env, &VaultDataKey::AssetToken)?;
        let share = get_config_address(&env, &VaultDataKey::ShareToken)?;
        if token == asset || token == share {
            return Err(ContractError::InvalidInput);
        }

        let recipient = get_config_address(&env, &VaultDataKey::SkimRecipient)?;
        let client = soroban_sdk::token::Client::new(&env, &token);
        let balance = client.balance(&env.current_contract_address());
        if balance <= 0 {
            return Err(ContractError::InvalidState);
        }

        client.transfer(&env.current_contract_address(), &recipient, &balance);
        emit_admin_event(&env, symbol_short!("skim"));
        Ok(())
    }

    /// Cancel a pending migration if migrate() has not been called.
    /// This reverts the vault to normal operation after a failed or abandoned upgrade.
    pub fn cancel_migration(env: Env, caller: soroban_sdk::Address) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &caller)?;

        if !stellar_contract_utils::upgradeable::can_complete_migration(&env) {
            return Err(ContractError::InvalidState);
        }

        // Clear the migration flag so normal operations resume.
        stellar_contract_utils::upgradeable::complete_migration(&env);

        emit_admin_event(&env, symbol_short!("cnc_migr"));
        Ok(())
    }

    pub fn config(
        env: Env,
    ) -> Result<
        (
            soroban_sdk::Address,
            soroban_sdk::Address,
            soroban_sdk::Address,
            soroban_sdk::Address,
        ),
        ContractError,
    > {
        ensure_not_reentrant(&env)?;
        Ok((
            get_config_address(&env, &VaultDataKey::Curator)?,
            get_config_address(&env, &VaultDataKey::Governance)?,
            get_config_address(&env, &VaultDataKey::AssetToken)?,
            get_config_address(&env, &VaultDataKey::ShareToken)?,
        ))
    }

    pub fn supply_queue(env: Env) -> Result<soroban_sdk::Vec<u32>, ContractError> {
        ensure_not_reentrant(&env)?;
        let mut queue = soroban_sdk::Vec::new(&env);
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            for target_id in vault.supply_queue_targets() {
                queue.push_back(target_id);
            }
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(queue)
    }

    pub fn is_paused(env: Env) -> Result<bool, ContractError> {
        ensure_not_reentrant(&env)?;
        let storage = SorobanStorage::new(&env);
        Ok(storage.is_paused())
    }

    pub fn vault_snapshot(env: Env) -> Result<(i128, i128, i128), ContractError> {
        ensure_not_reentrant(&env)?;
        Ok(query_vault_snapshot(&env))
    }

    pub fn fee_info(env: Env) -> Result<(i128, u64, i128, i128), ContractError> {
        ensure_not_reentrant(&env)?;
        let mut result: (i128, u64, i128, i128) = (0, 0, 0, 0);
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            let anchor = vault.get_fee_anchor()?;
            let fees = vault.get_fees();
            result = (
                anchor.total_assets as i128,
                anchor.timestamp_ns,
                u128::from(fees.management.fee_wad) as i128,
                u128::from(fees.performance.fee_wad) as i128,
            );
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(result)
    }

    pub fn cap_groups(
        env: Env,
    ) -> Result<soroban_sdk::Vec<(soroban_sdk::String, i128, i128)>, ContractError> {
        ensure_not_reentrant(&env)?;
        let mut groups = soroban_sdk::Vec::new(&env);
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            for (id, rec) in vault.get_cap_groups() {
                let sdk_id = soroban_sdk::String::from_str(&env, &id.0);
                let abs_cap = rec.cap.absolute_cap.map(|c| c.get() as i128).unwrap_or(0);
                groups.push_back((sdk_id, abs_cap, rec.principal as i128));
            }
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(groups)
    }

    pub fn queue_tail(env: Env) -> Result<u64, ContractError> {
        ensure_not_reentrant(&env)?;
        let mut result = 0u64;
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            result = vault.queue_tail()?;
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(result)
    }

    pub fn withdraw_status(env: Env) -> Result<(i64, i64, i64), ContractError> {
        ensure_not_reentrant(&env)?;
        let mut result: (i64, i64, i64) = (-1, -1, -1);
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            result.0 = vault
                .peek_next_pending_withdrawal_id()?
                .map(|id| id as i64)
                .unwrap_or(-1);
            result.1 = vault
                .get_withdrawing_op_id()?
                .map(|id| id as i64)
                .unwrap_or(-1);
            result.2 = vault
                .get_current_withdraw_request_id()?
                .map(|id| id as i64)
                .unwrap_or(-1);
            Ok(())
        };
        with_contract_vault_contract_error(&env, &mut call)?;
        Ok(result)
    }

    pub fn extend_ttl(env: Env) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        extend_storage_ttl(&env);
        Ok(())
    }

    pub fn upgrade(
        env: Env,
        new_wasm_hash: BytesN<32>,
        operator: soroban_sdk::Address,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_governance(&env, &operator)?;

        // Enable migration state before upgrading
        stellar_contract_utils::upgradeable::enable_migration(&env);

        // Replace contract code - takes effect after this invocation completes
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        emit_admin_event(&env, symbol_short!("upgrade"));
        Ok(())
    }

    pub fn migrate(env: Env, operator: soroban_sdk::Address) -> Result<(), ContractError> {
        use stellar_contract_utils::upgradeable::{
            complete_migration, ensure_can_complete_migration,
        };

        ensure_not_reentrant(&env)?;
        require_governance(&env, &operator)?;

        // Verify we're in migration state (upgrade was called)
        ensure_can_complete_migration(&env);

        // Run storage migrations
        migrate_legacy_paused(&env);

        // Extend TTL after migration
        extend_storage_ttl(&env);

        // Mark migration as complete - normal operations can resume
        complete_migration(&env);
        emit_admin_event(&env, symbol_short!("migrate"));
        Ok(())
    }

    pub fn is_migrating(env: Env) -> Result<bool, ContractError> {
        ensure_not_reentrant(&env)?;
        Ok(stellar_contract_utils::upgradeable::can_complete_migration(
            &env,
        ))
    }

    pub fn query_asset(env: Env) -> Result<soroban_sdk::Address, ContractError> {
        ensure_not_reentrant(&env)?;
        get_config_address(&env, &VaultDataKey::AssetToken)
    }

    pub fn total_assets(env: Env) -> Result<i128, ContractError> {
        ensure_not_reentrant(&env)?;
        Ok(query_vault_field(&env, |s| s.total_assets))
    }

    pub fn convert_to_shares(env: Env, assets: i128) -> Result<i128, ContractError> {
        ensure_not_reentrant(&env)?;
        if assets <= 0 {
            return Ok(0);
        }
        let (state, config) = load_state_and_config(&env)?;
        let assets_u128 = to_u128(assets)?;
        to_i128(convert_to_shares(&state, &config, assets_u128))
    }

    pub fn convert_to_assets(env: Env, shares: i128) -> Result<i128, ContractError> {
        ensure_not_reentrant(&env)?;
        if shares <= 0 {
            return Ok(0);
        }
        let (state, config) = load_state_and_config(&env)?;
        let shares_u128 = to_u128(shares)?;
        to_i128(convert_to_assets(&state, &config, shares_u128))
    }

    pub fn max_deposit(env: Env, _receiver: soroban_sdk::Address) -> Result<i128, ContractError> {
        max_deposit_or_mint(&env, false)
    }

    pub fn max_mint(env: Env, _receiver: soroban_sdk::Address) -> Result<i128, ContractError> {
        max_deposit_or_mint(&env, true)
    }

    pub fn max_withdraw(env: Env, owner: soroban_sdk::Address) -> Result<i128, ContractError> {
        max_withdraw_or_redeem(&env, &owner, false)
    }

    pub fn max_redeem(env: Env, owner: soroban_sdk::Address) -> Result<i128, ContractError> {
        max_withdraw_or_redeem(&env, &owner, true)
    }

    pub fn preview_deposit(env: Env, assets: i128) -> Result<i128, ContractError> {
        Self::convert_to_shares(env, assets)
    }

    pub fn preview_mint(env: Env, shares: i128) -> Result<i128, ContractError> {
        ensure_not_reentrant(&env)?;
        if shares <= 0 {
            return Ok(0);
        }
        let (state, config) = load_state_and_config(&env)?;
        let shares_u128 = to_u128(shares)?;
        to_i128(convert_to_assets_ceil(&state, &config, shares_u128))
    }

    pub fn preview_withdraw(env: Env, assets: i128) -> Result<i128, ContractError> {
        ensure_not_reentrant(&env)?;
        if assets <= 0 {
            return Ok(0);
        }
        let (state, config) = load_state_and_config(&env)?;
        let assets_u128 = to_u128(assets)?;
        to_i128(convert_to_shares_ceil(&state, &config, assets_u128))
    }

    pub fn preview_redeem(env: Env, shares: i128) -> Result<i128, ContractError> {
        Self::convert_to_assets(env, shares)
    }

    pub fn deposit(
        env: Env,
        assets: i128,
        receiver: soroban_sdk::Address,
        from: soroban_sdk::Address,
        operator: soroban_sdk::Address,
    ) -> Result<i128, ContractError> {
        require_signed(&operator);
        ensure_not_reentrant(&env)?;
        if assets <= 0 {
            return Err(ContractError::InvalidInput);
        }
        Self::deposit_with_min(env, from, receiver, assets, 0)
    }

    pub fn mint(
        env: Env,
        shares: i128,
        receiver: soroban_sdk::Address,
        from: soroban_sdk::Address,
        operator: soroban_sdk::Address,
    ) -> Result<i128, ContractError> {
        require_signed(&operator);
        ensure_not_reentrant(&env)?;
        if shares <= 0 {
            return Err(ContractError::InvalidInput);
        }
        let (state, config) = load_state_and_config(&env)?;
        let shares_u128 = to_u128(shares)?;
        let assets_needed = convert_to_assets_ceil(&state, &config, shares_u128);
        let assets_i128 = to_i128(assets_needed)?;
        Self::deposit_with_min(env, from, receiver, assets_i128, shares)?;
        Ok(assets_i128)
    }

    pub fn withdraw(
        env: Env,
        assets: i128,
        receiver: soroban_sdk::Address,
        owner: soroban_sdk::Address,
        operator: soroban_sdk::Address,
    ) -> Result<i128, ContractError> {
        atomic_withdraw_impl(&env, assets, &receiver, &owner, &operator, false)
    }

    pub fn redeem(
        env: Env,
        shares: i128,
        receiver: soroban_sdk::Address,
        owner: soroban_sdk::Address,
        operator: soroban_sdk::Address,
    ) -> Result<i128, ContractError> {
        atomic_withdraw_impl(&env, shares, &receiver, &owner, &operator, true)
    }
}

#[inline]
fn require_signed(addr: &SdkAddress) {
    addr.require_auth();
}

fn max_deposit_or_mint(env: &Env, use_shares: bool) -> Result<i128, ContractError> {
    ensure_not_reentrant(env)?;
    let (state, config) = load_state_and_config(env)?;
    if state.op_state.is_idle() && !config.paused {
        let total = if use_shares {
            state.total_shares
        } else {
            state.total_assets
        };
        let remaining = u128::MAX.saturating_sub(total);
        Ok(remaining.min(i128::MAX as u128) as i128)
    } else {
        Ok(0)
    }
}

fn max_withdraw_or_redeem(
    env: &Env,
    owner: &SdkAddress,
    is_redeem: bool,
) -> Result<i128, ContractError> {
    ensure_not_reentrant(env)?;
    let (state, config) = load_state_and_config(env)?;
    if !state.op_state.is_idle() {
        return Ok(0);
    }
    let owner_shares = share_balance(env, owner).max(0) as u128;
    let max = if is_redeem {
        let shares_from_idle = convert_to_shares(&state, &config, state.idle_assets);
        owner_shares.min(shares_from_idle)
    } else {
        let assets_from_shares = convert_to_assets(&state, &config, owner_shares);
        assets_from_shares.min(state.idle_assets)
    };
    Ok(i128::try_from(max).unwrap_or(0))
}

fn require_governance(env: &Env, caller: &SdkAddress) -> Result<(), ContractError> {
    require_signed(caller);
    let governance: SdkAddress = get_config_address(env, &VaultDataKey::Governance)?;
    if caller != &governance {
        return Err(ContractError::Unauthorized);
    }
    Ok(())
}

/// Shared implementation for SEP-41 `withdraw` (by assets) and `redeem` (by shares).
fn atomic_withdraw_impl(
    env: &Env,
    amount: i128,
    receiver: &SdkAddress,
    owner: &SdkAddress,
    operator: &SdkAddress,
    is_redeem: bool,
) -> Result<i128, ContractError> {
    require_signed(operator);
    require_signed(owner);
    if amount <= 0 {
        return Err(ContractError::InvalidInput);
    }
    let mut result = 0u128;
    with_reentrancy_guard(env, &mut || {
        let amount_u128 = to_u128(amount)?;
        let (state, _config) = load_state_and_config(env)?;
        if !state.op_state.is_idle() {
            return Err(ContractError::VaultNotIdle);
        }
        if !is_redeem && amount_u128 > state.idle_assets {
            return Err(ContractError::InsufficientIdleAssets);
        }
        refresh_fees_for_atomic(env)?;
        let (state, config) = load_state_and_config(env)?;
        let (assets, shares) = if is_redeem {
            let assets_out = convert_to_assets(&state, &config, amount_u128);
            if assets_out > state.idle_assets {
                return Err(ContractError::InsufficientIdleAssets);
            }
            (assets_out, amount_u128)
        } else {
            (
                amount_u128,
                convert_to_shares_ceil(&state, &config, amount_u128),
            )
        };
        atomic_withdraw_internal(env, owner, receiver, assets, shares)?;
        result = if is_redeem { assets } else { shares };
        Ok(())
    })?;
    to_i128(result)
}

/// Invoke adapter.{fn_name}(vault, asset, amount).
fn call_adapter(env: &Env, adapter: &SdkAddress, fn_name: &str, asset: &SdkAddress, amount: i128) {
    let vault = env.current_contract_address();
    let name = Symbol::new(env, fn_name);
    let args: soroban_sdk::Vec<Val> = (vault, asset.clone(), amount).into_val(env);
    env.invoke_contract::<Val>(adapter, &name, args);
}

/// Shared preamble for governance entrypoints: auth check + curator kernel address.
#[inline(never)]
fn governance_caller(env: &Env, caller: &SdkAddress) -> Result<Address, ContractError> {
    require_governance(env, caller)?;
    Ok(kernel_address_from_sdk(env, caller))
}

#[cfg(test)]
mod tests;
