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

use crate::fungible_vault::{
    atomic_withdraw_internal, load_state_and_config, refresh_fees_for_atomic, share_balance,
    to_i128, to_u128,
};
use alloc::vec;
use alloc::vec::Vec;
use soroban_sdk::{
    contract, contractimpl, contracttype, panic_with_error, Address as SdkAddress, Bytes, BytesN,
    Env,
};
use templar_curator_primitives::{
    determine_recovery_action, PolicyState, RecoveryContext, RecoveryProgress,
};
use templar_vault_kernel::effects::{KernelEffect, KernelEvent};
use templar_vault_kernel::state::queue::{compute_full_withdrawal, DEFAULT_COOLDOWN_NS};
use templar_vault_kernel::{
    apply_action, complete_allocation, complete_refresh, compute_fee_shares_from_assets,
    convert_to_assets, convert_to_assets_ceil, convert_to_shares, convert_to_shares_ceil,
    mul_div_floor, start_allocation, start_refresh, withdrawal_collected, withdrawal_step_callback,
    Address, AssetId, FeeAccrualAnchor, FeesSpec, KernelAction, Number, OpState, PayoutOutcome,
    Restrictions, TargetId, VaultConfig, VaultState, MAX_PENDING, MIN_WITHDRAWAL_ASSETS,
};

use crate::auth::{ActionKind, AuthAdapter};
use crate::effects::{
    AddressRegistrar, EffectContext, EffectInterpreter, EffectSummary, SdkTokenAdapter,
    SorobanEffectInterpreter,
};
use crate::error::{ContractError, RuntimeError};
use crate::market::{CrossChainMarketAdapter, MarketAdapter, MarketRef};
use crate::policy::{build_refresh_plan_with_locks, filter_allocation_plan};
use crate::reconciliation::{reconcile_external_assets, ReconciliationRecord};
use crate::storage::{SorobanStorage, Storage, VersionedState};
use templar_curator_primitives::rbac::{RbacAuth, RbacConfig};

const ESCROW_ADDRESS: Address = [0u8; 32];
const KERNEL_ADDRESS_DOMAIN: &[u8] = b"templar:soroban:address";
const YEAR_NS: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;
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

fn ledger_timestamp_ns(env: &Env) -> u64 {
    env.ledger().timestamp().saturating_mul(1_000_000_000)
}

fn is_contract_address(addr: &SdkAddress) -> bool {
    let bytes = addr.to_string().to_bytes();
    matches!(bytes.get(0), Some(b'C'))
}

fn require_contract_address(addr: &SdkAddress, msg: &'static str) -> Result<(), ContractError> {
    if is_contract_address(addr) {
        Ok(())
    } else {
        Err(RuntimeError::invalid_input(msg).into())
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct NoopMarketAdapter;

impl MarketAdapter for NoopMarketAdapter {
    fn supply(&mut self, _market: MarketRef, _amount: u128) -> Result<(), RuntimeError> {
        Err(RuntimeError::contract_error(
            "market adapter not configured",
        ))
    }

    fn withdraw(&mut self, _market: MarketRef, _amount: u128) -> Result<(), RuntimeError> {
        Err(RuntimeError::contract_error(
            "market adapter not configured",
        ))
    }

    fn total_assets(&self, _market: MarketRef) -> Result<u128, RuntimeError> {
        Err(RuntimeError::contract_error(
            "market adapter not configured",
        ))
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct NoopCrossChainAdapter;

impl CrossChainMarketAdapter for NoopCrossChainAdapter {
    fn submit_intent(
        &mut self,
        _plan_bytes: Vec<u8>,
    ) -> Result<crate::market::AttemptId, RuntimeError> {
        Err(RuntimeError::contract_error(
            "cross-chain adapter not configured",
        ))
    }

    fn settle(
        &mut self,
        _op_id: u64,
        _attempt_id: crate::market::AttemptId,
    ) -> Result<crate::market::SettlementReceipt, RuntimeError> {
        Err(RuntimeError::contract_error(
            "cross-chain adapter not configured",
        ))
    }

    fn total_assets(&self, _market: MarketRef) -> Result<u128, RuntimeError> {
        Err(RuntimeError::contract_error(
            "cross-chain adapter not configured",
        ))
    }
}

fn serialize_fees_spec(fees: &FeesSpec) -> Result<Vec<u8>, RuntimeError> {
    borsh::to_vec(fees).map_err(|_| RuntimeError::storage_error("fees serialize failed"))
}

fn deserialize_fees_spec(bytes: &[u8]) -> Result<FeesSpec, RuntimeError> {
    <FeesSpec as borsh::BorshDeserialize>::try_from_slice(bytes)
        .map_err(|_| RuntimeError::storage_error("fees deserialize failed"))
}

pub(crate) fn load_fees_spec(env: &Env) -> Result<FeesSpec, RuntimeError> {
    let stored: Option<Vec<u8>> = env.storage().instance().get(&VaultDataKey::FeesSpec);
    match stored {
        Some(bytes) => deserialize_fees_spec(&bytes),
        None => Ok(FeesSpec::zero()),
    }
}

fn store_fees_spec(env: &Env, fees: &FeesSpec) -> Result<(), RuntimeError> {
    let bytes = serialize_fees_spec(fees)?;
    env.storage()
        .instance()
        .set(&VaultDataKey::FeesSpec, &bytes);
    Ok(())
}

/// Contract configuration set at initialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractConfig {
    /// Curator address.
    pub curator: Address,
    /// Vault contract address.
    pub vault_address: Address,
    /// Guardian addresses (can pause).
    pub guardians: Vec<Address>,
    /// Allocator addresses (can manage allocations).
    pub allocators: Vec<Address>,
    /// Underlying asset contract address.
    pub asset_address: Address,
    /// Share token contract address.
    pub share_address: Address,
    /// Fee configuration.
    pub fees: FeesSpec,
    /// Blend adapter contract address (optional).
    pub blend_adapter: Option<Address>,
    /// Blend pool contract address (optional).
    pub blend_pool: Option<Address>,
    /// Blend factory contract address (optional).
    pub blend_factory: Option<Address>,
}

impl ContractConfig {
    /// Create a new contract configuration.
    #[inline]
    #[must_use]
    pub fn new(
        curator: Address,
        vault_address: Address,
        guardians: Vec<Address>,
        allocators: Vec<Address>,
        asset_address: Address,
        share_address: Address,
    ) -> Self {
        Self {
            curator,
            vault_address,
            guardians,
            allocators,
            asset_address,
            share_address,
            fees: FeesSpec::zero(),
            blend_adapter: None,
            blend_pool: None,
            blend_factory: None,
        }
    }

    /// Attach a Blend adapter contract address.
    #[inline]
    #[must_use]
    pub fn with_blend_adapter(mut self, adapter: Address) -> Self {
        self.blend_adapter = Some(adapter);
        self
    }

    /// Attach a Blend pool contract address.
    #[inline]
    #[must_use]
    pub fn with_blend_pool(mut self, pool: Address) -> Self {
        self.blend_pool = Some(pool);
        self
    }

    /// Attach a Blend factory contract address.
    #[inline]
    #[must_use]
    pub fn with_blend_factory(mut self, factory: Address) -> Self {
        self.blend_factory = Some(factory);
        self
    }

    /// Attach a fees configuration.
    #[inline]
    #[must_use]
    pub fn with_fees(mut self, fees: FeesSpec) -> Self {
        self.fees = fees;
        self
    }

    /// Check if the given address is the curator.
    #[inline]
    #[must_use]
    pub fn is_curator(&self, addr: &Address) -> bool {
        &self.curator == addr
    }

    /// Check if the given address is a guardian.
    #[inline]
    #[must_use]
    pub fn is_guardian(&self, addr: &Address) -> bool {
        self.guardians.iter().any(|g| g == addr)
    }

    /// Check if the given address is an allocator.
    #[inline]
    #[must_use]
    pub fn is_allocator(&self, addr: &Address) -> bool {
        self.allocators.iter().any(|a| a == addr)
    }

    /// Check if the address has privileged access (curator or allocator).
    #[inline]
    #[must_use]
    pub fn is_privileged(&self, addr: &Address) -> bool {
        self.is_curator(addr) || self.is_allocator(addr)
    }
}

/// Deposit result returned to caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepositResult {
    /// Shares minted to the receiver.
    pub shares_minted: u128,
    /// New total shares.
    pub total_shares: u128,
    /// New total assets.
    pub total_assets: u128,
}

/// Withdrawal request result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawRequestResult {
    /// The withdrawal queue position/ID.
    pub request_id: u64,
    /// Shares escrowed for this withdrawal.
    pub shares_escrowed: u128,
}

/// Allocation result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocationResult {
    /// Operation ID.
    pub op_id: u64,
    /// New external assets after allocation.
    pub new_external_assets: u128,
    /// Effect summary from executing effects.
    pub summary: EffectSummary,
}

/// Refresh result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshResult {
    /// Operation ID.
    pub op_id: u64,
    /// Markets refreshed count.
    pub markets_refreshed: u32,
    /// New external assets discovered.
    pub new_external_assets: u128,
}

/// Curator vault contract.
///
/// This struct wraps the vault state, storage, auth, effect interpreter,
/// and market adapters to provide the full contract interface.
pub struct CuratorVault<S, A, E, M, C>
where
    S: Storage,
    A: AuthAdapter,
    E: EffectInterpreter + AddressRegistrar,
    M: MarketAdapter,
    C: CrossChainMarketAdapter,
{
    /// Contract configuration.
    pub config: ContractConfig,
    /// Storage backend.
    pub storage: S,
    /// Auth adapter.
    pub auth: A,
    /// Effect interpreter.
    pub interpreter: E,
    /// Local market adapter.
    pub market: M,
    /// Cross-chain market adapter.
    pub cross_chain: C,
    /// Vault state (loaded from storage).
    state: Option<VaultState>,
    /// Policy state (locks, caps, supply queue).
    policy_state: PolicyState,
    /// Optional kernel restrictions (pause/allowlist/denylist).
    restrictions: Option<Restrictions>,
    /// Whether the vault is paused.
    paused: bool,
}

impl<S, A, E, M, C> CuratorVault<S, A, E, M, C>
where
    S: Storage,
    A: AuthAdapter,
    E: EffectInterpreter + AddressRegistrar,
    M: MarketAdapter,
    C: CrossChainMarketAdapter,
{
    /// Create a new curator vault instance.
    #[inline]
    #[must_use]
    pub fn new(
        config: ContractConfig,
        storage: S,
        auth: A,
        interpreter: E,
        market: M,
        cross_chain: C,
    ) -> Self {
        Self {
            config,
            storage,
            auth,
            interpreter,
            market,
            cross_chain,
            state: None,
            policy_state: PolicyState::new(),
            restrictions: None,
            paused: false,
        }
    }

    /// Initialize or load vault state from storage.
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

    fn authorize_and_apply(
        &mut self,
        kind: ActionKind,
        caller: Address,
        action: KernelAction,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        self.auth.authorize(kind, caller, None)?;
        self.apply_kernel_action(action, now_ns)
    }

    fn begin_operation<F>(
        &mut self,
        kind: ActionKind,
        caller: Address,
        apply: F,
    ) -> Result<u64, RuntimeError>
    where
        F: FnOnce(&mut VaultState, u64) -> Result<OpState, RuntimeError>,
    {
        self.auth.authorize(kind, caller, None)?;
        let state = self.state_mut()?;
        let op_id = state.next_op_id;
        state.next_op_id = state.next_op_id.saturating_add(1);
        let next_op_state = apply(state, op_id)?;
        state.op_state = next_op_state;
        self.save_state()?;
        Ok(op_id)
    }

    fn finish_operation<R, F>(
        &mut self,
        kind: ActionKind,
        caller: Address,
        op_id: u64,
        apply: F,
    ) -> Result<R, RuntimeError>
    where
        F: FnOnce(&mut VaultState, u64) -> Result<R, RuntimeError>,
    {
        self.auth.authorize(kind, caller, None)?;
        let result = {
            let state = self.state_mut()?;
            apply(state, op_id)?
        };
        self.save_state()?;
        Ok(result)
    }

    /// Get a reference to the current vault state.
    #[inline]
    pub fn state(&self) -> Result<&VaultState, RuntimeError> {
        self.state
            .as_ref()
            .ok_or_else(|| RuntimeError::storage_error("vault state not loaded"))
    }

    /// Get a mutable reference to the current vault state.
    #[inline]
    pub fn state_mut(&mut self) -> Result<&mut VaultState, RuntimeError> {
        self.state
            .as_mut()
            .ok_or_else(|| RuntimeError::storage_error("vault state not loaded"))
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

    fn total_assets_for_fee_accrual(
        &self,
        cur_total_assets: u128,
        anchor: &FeeAccrualAnchor,
        now_ns: u64,
    ) -> Result<u128, RuntimeError> {
        let Some(max_rate) = self.config.fees.max_total_assets_growth_rate else {
            return Ok(cur_total_assets);
        };

        let anchor_assets = anchor.total_assets;
        if cur_total_assets <= anchor_assets || anchor_assets == 0 || now_ns < anchor.timestamp_ns {
            return Ok(cur_total_assets);
        }

        let elapsed_ns = now_ns - anchor.timestamp_ns;
        if elapsed_ns == 0 {
            return Ok(anchor_assets);
        }

        let annual_max_increase = max_rate.apply_floored(Number::from(anchor_assets));
        let max_increase = mul_div_floor(
            annual_max_increase,
            Number::from(u128::from(elapsed_ns)),
            Number::from(u128::from(YEAR_NS)),
        )
        .as_u128_saturating();

        let max_total_assets = anchor_assets
            .checked_add(max_increase)
            .ok_or_else(|| RuntimeError::contract_error("fee accrual overflow"))?;
        Ok(cur_total_assets.min(max_total_assets))
    }

    fn compute_management_fee_shares(
        &self,
        fee_assets_base: u128,
        cur_total_assets: u128,
        total_supply: u128,
        last_timestamp_ns: u64,
        now_ns: u64,
    ) -> Number {
        if self.config.fees.management.fee_wad.is_zero()
            || total_supply == 0
            || now_ns <= last_timestamp_ns
        {
            return Number::zero();
        }
        let elapsed_ns = now_ns - last_timestamp_ns;
        let annual_fee_assets = self
            .config
            .fees
            .management
            .fee_wad
            .apply_floored(Number::from(fee_assets_base));
        let fee_assets = mul_div_floor(
            annual_fee_assets,
            Number::from(u128::from(elapsed_ns)),
            Number::from(u128::from(YEAR_NS)),
        );
        compute_fee_shares_from_assets(
            fee_assets,
            Number::from(cur_total_assets),
            Number::from(total_supply),
        )
    }

    fn apply_kernel_action(
        &mut self,
        action: KernelAction,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        let config = self.kernel_config();
        let restrictions = self.restrictions.as_ref();
        let state = self.state()?.clone();
        let result = apply_action(
            state,
            &config,
            restrictions,
            &self.config.vault_address,
            action,
        )
        .map_err(RuntimeError::transition_error)?;

        let ctx = self.effect_context(now_ns);
        self.ensure_effect_addresses_mapped(&result.effects, &ctx)?;
        let summary = self.interpreter.execute_effects(&result.effects, &ctx)?;

        self.state = Some(result.state);
        self.save_state()?;

        Ok(summary)
    }

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

    // =========================================================================
    // User-facing entrypoints
    // =========================================================================

    /// Deposit assets into the vault.
    ///
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
            return Err(RuntimeError::contract_error("vault is paused"));
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

    /// Deposit using Soroban addresses, registering mappings automatically.
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
    ///
    /// This queues a withdrawal request. The actual withdrawal will be processed
    /// when `execute_withdraw` is called.
    ///
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
            return Err(RuntimeError::contract_error("no shares in vault"));
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

    /// Request withdrawal using Soroban addresses, registering mappings automatically.
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
    ///
    /// This processes the next pending withdrawal in the queue.
    ///
    pub fn execute_withdraw(
        &mut self,
        caller: Address,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::ExecuteWithdraw, caller, None)?;

        let mut summary = EffectSummary::new();

        let op_state = self.state()?.op_state.clone();
        if op_state.is_idle() {
            let step_summary =
                self.apply_kernel_action(KernelAction::ExecuteWithdraw { now_ns }, now_ns)?;
            summary.merge(step_summary);
        } else if !op_state.is_withdrawing() {
            return Err(RuntimeError::contract_error(
                "vault not in idle or withdrawing state for withdrawal",
            ));
        }

        if self.state()?.op_state.is_withdrawing() {
            let settle_summary = self.complete_withdrawal_from_idle(now_ns)?;
            summary.merge(settle_summary);
        }

        Ok(summary)
    }

    /// Execute withdrawal using a Soroban address, registering mappings automatically.
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

    fn complete_withdrawal_from_idle(
        &mut self,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        let (_, pending) = self
            .state()?
            .withdraw_queue
            .head()
            .ok_or_else(|| RuntimeError::contract_error("withdraw queue empty"))?;
        let pending = pending.clone();

        let withdraw = match self.state()?.op_state.clone() {
            OpState::Withdrawing(state) => state,
            _ => return Err(RuntimeError::contract_error("withdrawal not in progress")),
        };

        if pending.owner != withdraw.owner
            || pending.receiver != withdraw.receiver
            || pending.escrow_shares != withdraw.escrow_shares
        {
            return Err(RuntimeError::contract_error(
                "withdrawal queue head mismatch",
            ));
        }

        let available_assets = self.state()?.idle_assets;
        if available_assets == 0 {
            return Ok(EffectSummary::new());
        }

        if available_assets < pending.expected_assets {
            return Ok(EffectSummary::new());
        }

        let withdrawal_result = compute_full_withdrawal(&pending, available_assets)
            .ok_or_else(|| RuntimeError::contract_error("withdrawal not satisfiable"))?;

        let assets_out = withdrawal_result.assets_out;
        if assets_out == 0 {
            return Ok(EffectSummary::new());
        }

        let burn_shares = withdrawal_result.settlement.to_burn;
        let refund_shares = withdrawal_result.settlement.refund;
        let op_id = withdraw.op_id;

        let step = {
            let op_state = self.state()?.op_state.clone();
            withdrawal_step_callback(op_state, op_id, assets_out)
                .map_err(RuntimeError::transition_error)?
        };
        self.state_mut()?.op_state = step.new_state;

        let collected = {
            let op_state = self.state()?.op_state.clone();
            withdrawal_collected(op_state, op_id, burn_shares)
                .map_err(RuntimeError::transition_error)?
        };
        let ctx = self.effect_context(now_ns);
        self.ensure_effect_addresses_mapped(&collected.effects, &ctx)?;
        let mut summary = self.interpreter.execute_effects(&collected.effects, &ctx)?;
        self.state_mut()?.op_state = collected.new_state;

        let payout = match &self.state()?.op_state {
            OpState::Payout(state) => state,
            _ => {
                return Err(RuntimeError::contract_error(
                    "expected payout state after withdrawal",
                ))
            }
        };

        let transfer_effects = [KernelEffect::TransferAssets {
            to: payout.receiver,
            amount: assets_out,
        }];
        self.ensure_effect_addresses_mapped(&transfer_effects, &ctx)?;
        let transfer_summary = self.interpreter.execute_effects(&transfer_effects, &ctx)?;
        summary.merge(transfer_summary);

        let state = self.state_mut()?;
        state.idle_assets = state.idle_assets.saturating_sub(assets_out);
        state.total_assets = state.idle_assets.saturating_add(state.external_assets);

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

    // =========================================================================
    // Privileged entrypoints (internal/runtime)
    // =========================================================================

    /// Begin an allocation operation.
    ///
    /// Filters the plan to exclude locked markets before starting.
    ///
    pub fn begin_allocating(
        &mut self,
        caller: Address,
        plan: Vec<(TargetId, u128)>,
        current_ns: u64,
    ) -> Result<u64, RuntimeError> {
        // Filter plan to exclude locked markets
        let filtered_plan = filter_allocation_plan(&plan, &self.policy_state.locks, current_ns);

        self.begin_operation(ActionKind::BeginAllocating, caller, move |state, op_id| {
            let alloc_total: u128 = filtered_plan.iter().map(|(_, amt)| *amt).sum();
            if alloc_total > state.idle_assets {
                return Err(RuntimeError::insufficient_balance(
                    state.idle_assets,
                    alloc_total,
                ));
            }
            state.idle_assets -= alloc_total;
            state.total_assets = state.idle_assets.saturating_add(state.external_assets);

            let result = start_allocation(state.op_state.clone(), filtered_plan, op_id)
                .map_err(RuntimeError::transition_error)?;
            Ok(result.new_state)
        })
    }

    /// Sync external assets during an operation.
    ///
    /// Verifies the caller's value against market adapter balances when
    /// the active operation has a plan with targets (Allocating/Refreshing).
    pub fn sync_external_assets(
        &mut self,
        caller: Address,
        new_external_assets: u128,
        op_id: u64,
        now_ns: u64,
    ) -> Result<(), RuntimeError> {
        self.verify_external_assets_against_adapter(new_external_assets)?;

        let _summary = self.authorize_and_apply(
            ActionKind::SyncExternalAssets,
            caller,
            KernelAction::SyncExternalAssets {
                new_external_assets,
                op_id,
                now_ns,
            },
            now_ns,
        )?;

        Ok(())
    }

    /// Verify the claimed external_assets against adapter-reported balances.
    ///
    /// Only performs an exact match during **refresh** operations, where the
    /// plan covers all markets and the adapter total should equal the claimed
    /// external_assets. During allocation, the plan only covers target markets
    /// while `new_external_assets` includes non-plan markets too, so an exact
    /// match is not possible — we fall through to the kernel's 2x bounds check.
    ///
    /// If the adapter is not configured (all queries fail), verification is
    /// skipped. If some queries succeed and others fail (partial failure),
    /// the call is rejected rather than silently accepting an unchecked value.
    fn verify_external_assets_against_adapter(&self, claimed: u128) -> Result<(), RuntimeError> {
        let state = self.state()?;

        // Only verify during refresh (plan covers all markets).
        // During allocation, plan covers only target markets — can't do exact match.
        let targets: Vec<TargetId> = match &state.op_state {
            OpState::Refreshing(s) => s.plan.clone(),
            _ => return Ok(()),
        };

        if targets.is_empty() {
            return Ok(());
        }

        // Query adapter for each target's balance, tracking successes and failures
        let asset_id = AssetId::from(self.config.asset_address);
        let mut adapter_total: u128 = 0;
        let mut ok_count: usize = 0;
        let mut failed_targets: Vec<TargetId> = Vec::new();
        for target_id in &targets {
            match self
                .market
                .total_assets(MarketRef::new(*target_id, asset_id.clone()))
            {
                Ok(balance) => {
                    adapter_total = adapter_total.saturating_add(balance);
                    ok_count += 1;
                }
                Err(_) => {
                    failed_targets.push(*target_id);
                }
            }
        }

        if ok_count == 0 {
            return Err(RuntimeError::contract_error(
                "sync_external_assets: adapter unavailable for refresh verification",
            ));
        }

        if !failed_targets.is_empty() {
            // Partial failure: some targets succeeded, others failed. Reject to
            // prevent accepting an unverifiable value.
            return Err(RuntimeError::contract_error(alloc::format!(
                "sync_external_assets: adapter query failed for markets {:?}",
                failed_targets
            )));
        }

        if claimed != adapter_total {
            return Err(RuntimeError::contract_error(
                "sync_external_assets: claimed value does not match adapter-reported balances",
            ));
        }

        Ok(())
    }

    /// Finish an allocation operation.
    ///
    pub fn finish_allocating(
        &mut self,
        caller: Address,
        op_id: u64,
    ) -> Result<AllocationResult, RuntimeError> {
        self.finish_operation(
            ActionKind::FinishAllocating,
            caller,
            op_id,
            |state, op_id| {
                let transition_result = complete_allocation(state.op_state.clone(), op_id, None)
                    .map_err(RuntimeError::transition_error)?;
                state.op_state = transition_result.new_state;
                Ok(AllocationResult {
                    op_id,
                    new_external_assets: state.external_assets,
                    summary: EffectSummary::new(),
                })
            },
        )
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

        self.begin_operation(ActionKind::BeginRefreshing, caller, move |state, op_id| {
            let result = start_refresh(state.op_state.clone(), filtered_plan, op_id)
                .map_err(RuntimeError::transition_error)?;
            Ok(result.new_state)
        })
    }

    /// Finish a refresh operation.
    ///
    pub fn finish_refreshing(
        &mut self,
        caller: Address,
        op_id: u64,
    ) -> Result<RefreshResult, RuntimeError> {
        self.finish_operation(
            ActionKind::FinishRefreshing,
            caller,
            op_id,
            |state, op_id| {
                let markets_refreshed = match &state.op_state {
                    OpState::Refreshing(refresh) => refresh.plan.len() as u32,
                    _ => 0,
                };

                let result = complete_refresh(state.op_state.clone(), op_id)
                    .map_err(RuntimeError::transition_error)?;

                state.op_state = result.new_state;

                Ok(RefreshResult {
                    op_id,
                    markets_refreshed,
                    new_external_assets: state.external_assets,
                })
            },
        )
    }

    /// Abort an allocation operation.
    ///
    pub fn abort_allocating(
        &mut self,
        caller: Address,
        op_id: u64,
        restore_idle: u128,
    ) -> Result<(), RuntimeError> {
        self.authorize_and_apply(
            ActionKind::AbortAllocating,
            caller,
            KernelAction::AbortAllocating {
                op_id,
                restore_idle,
            },
            0,
        )
        .map(|_| ())
    }

    /// Abort a refresh operation.
    ///
    pub fn abort_refreshing(&mut self, caller: Address, op_id: u64) -> Result<(), RuntimeError> {
        self.authorize_and_apply(
            ActionKind::AbortRefreshing,
            caller,
            KernelAction::AbortRefreshing { op_id },
            0,
        )
        .map(|_| ())
    }

    /// Abort a withdrawal operation.
    ///
    pub fn abort_withdrawing(
        &mut self,
        caller: Address,
        op_id: u64,
        refund_shares: u128,
    ) -> Result<(), RuntimeError> {
        self.authorize_and_apply(
            ActionKind::AbortWithdrawing,
            caller,
            KernelAction::AbortWithdrawing {
                op_id,
                refund_shares,
            },
            0,
        )
        .map(|_| ())
    }

    /// Settle a payout operation.
    ///
    pub fn settle_payout(
        &mut self,
        caller: Address,
        op_id: u64,
        outcome: PayoutOutcome,
    ) -> Result<(), RuntimeError> {
        self.authorize_and_apply(
            ActionKind::SettlePayout,
            caller,
            KernelAction::SettlePayout { op_id, outcome },
            0,
        )
        .map(|_| ())
    }

    /// Recover from a stuck operation by delegating to curator-primitives.
    ///
    /// Returns `Ok(None)` if no recovery action is needed.
    pub fn recover(
        &mut self,
        caller: Address,
        context: RecoveryContext,
        progress: RecoveryProgress,
    ) -> Result<Option<EffectSummary>, RuntimeError> {
        let state = self.state()?;
        let Some(action) = determine_recovery_action(&state.op_state, &context, &progress) else {
            return Ok(None);
        };

        let kind: ActionKind = (&action).into();
        let summary = self.authorize_and_apply(kind, caller, action, context.current_ns)?;
        Ok(Some(summary))
    }

    /// Manual reconciliation of external assets.
    ///
    /// This is a privileged entrypoint that runs a full refresh cycle in one call:
    /// BeginRefreshing -> read principals -> SyncExternalAssets -> FinishRefreshing
    ///
    pub fn manual_reconcile(
        &mut self,
        caller: Address,
        markets: Vec<MarketRef>,
        now_ns: u64,
    ) -> Result<ReconciliationRecord, RuntimeError> {
        // Authorize - requires ManualReconcile privilege
        self.auth
            .authorize(ActionKind::ManualReconcile, caller, None)?;

        let plan: Vec<TargetId> = markets.iter().map(|m| m.market_id).collect();
        let op_id = self.begin_refreshing(caller, plan, now_ns)?;

        let refresh_markets: Vec<MarketRef> = match &self.state()?.op_state {
            OpState::Refreshing(refresh) => refresh
                .plan
                .iter()
                .copied()
                .map(|market_id| {
                    MarketRef::new(market_id, AssetId::from(self.config.asset_address))
                })
                .collect(),
            _ => {
                return Err(RuntimeError::contract_error(
                    "manual reconcile requires refreshing state",
                ))
            }
        };

        let record = reconcile_external_assets(&self.market, op_id, &refresh_markets)?;
        self.sync_external_assets(caller, record.new_external_assets, op_id, now_ns)?;
        let _result = self.finish_refreshing(caller, op_id)?;

        // Phase 4: Emit audit event
        let ctx = self.effect_context(now_ns);
        let effect = templar_vault_kernel::effects::KernelEffect::EmitEvent {
            event: templar_vault_kernel::effects::KernelEvent::RefreshCompleted { op_id },
        };
        self.interpreter.execute_effect(&effect, &ctx)?;

        Ok(record)
    }

    /// Refresh fees based on elapsed time.
    ///
    pub fn refresh_fees(&mut self, caller: Address, now_ns: u64) -> Result<u128, RuntimeError> {
        // Authorize
        self.auth.authorize(ActionKind::RefreshFees, caller, None)?;

        let state = self.state()?.clone();
        let anchor = state.fee_anchor;
        let cur_total_assets = state.total_assets;
        let mut total_supply = state.total_shares;

        let fee_total_assets =
            self.total_assets_for_fee_accrual(cur_total_assets, &anchor, now_ns)?;

        let mut next_state = state.clone();
        let mut effects = Vec::new();

        let management_shares = self.compute_management_fee_shares(
            fee_total_assets,
            cur_total_assets,
            total_supply,
            anchor.timestamp_ns,
            now_ns,
        );
        if !management_shares.is_zero() {
            let management_shares_u128 = u128::from(management_shares);
            effects.push(KernelEffect::MintShares {
                owner: self.config.fees.management.recipient,
                shares: management_shares_u128,
            });
            total_supply = total_supply
                .checked_add(management_shares_u128)
                .ok_or_else(|| RuntimeError::contract_error("management fee overflow"))?;
            next_state.total_shares = total_supply;
        }

        let profit = fee_total_assets.saturating_sub(anchor.total_assets);
        let fee_assets = self
            .config
            .fees
            .performance
            .fee_wad
            .apply_floored(Number::from(profit));
        let performance_shares = compute_fee_shares_from_assets(
            fee_assets,
            Number::from(cur_total_assets),
            Number::from(total_supply),
        );
        if !performance_shares.is_zero() {
            let performance_shares_u128 = u128::from(performance_shares);
            effects.push(KernelEffect::MintShares {
                owner: self.config.fees.performance.recipient,
                shares: performance_shares_u128,
            });
            total_supply = total_supply
                .checked_add(performance_shares_u128)
                .ok_or_else(|| RuntimeError::contract_error("performance fee overflow"))?;
            next_state.total_shares = total_supply;
        }

        next_state.fee_anchor = FeeAccrualAnchor::new(cur_total_assets, now_ns);

        effects.push(KernelEffect::EmitEvent {
            event: KernelEvent::FeesRefreshed {
                now_ns,
                total_assets: cur_total_assets,
            },
        });

        let ctx = self.effect_context(now_ns);
        self.ensure_effect_addresses_mapped(&effects, &ctx)?;
        let summary = self.interpreter.execute_effects(&effects, &ctx)?;

        self.state = Some(next_state);
        self.save_state()?;

        Ok(summary.shares_minted)
    }

    // =========================================================================
    // Policy management methods
    // =========================================================================

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

    /// Maximum lock duration: 7 days in nanoseconds.
    const MAX_LOCK_DURATION_NS: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;

    /// Acquire a market lock.
    ///
    pub fn acquire_market_lock(
        &mut self,
        caller: Address,
        target_id: TargetId,
        expiry_ns: u64,
        current_ns: u64,
    ) -> Result<(), RuntimeError> {
        use crate::policy::{validate_lock_expiry, MarketLock};

        // Authorize - requires allocator privileges
        self.auth
            .authorize(ActionKind::BeginAllocating, caller, None)?;

        if !validate_lock_expiry(current_ns, expiry_ns, Self::MAX_LOCK_DURATION_NS) {
            return Err(RuntimeError::contract_error("invalid market lock expiry"));
        }

        let lock = MarketLock::new(target_id, current_ns).with_expiry(expiry_ns);
        let new_locks = self
            .policy_state
            .locks
            .clone()
            .acquire(lock, current_ns)
            .map_err(|e| {
                RuntimeError::contract_error(alloc::format!("failed to acquire lock: {:?}", e))
            })?;
        let mut next_policy = self.policy_state.clone();
        next_policy.locks = new_locks;
        self.storage.save_policy_state(&next_policy)?;
        self.policy_state = next_policy;

        Ok(())
    }

    /// Release a market lock.
    ///
    pub fn release_market_lock(
        &mut self,
        caller: Address,
        target_id: TargetId,
    ) -> Result<(), RuntimeError> {
        // Authorize - requires allocator privileges
        self.auth
            .authorize(ActionKind::BeginAllocating, caller, None)?;

        let new_locks = self.policy_state.locks.clone().release(target_id);
        let mut next_policy = self.policy_state.clone();
        next_policy.locks = new_locks;
        self.storage.save_policy_state(&next_policy)?;
        self.policy_state = next_policy;

        Ok(())
    }

    /// Check if a market is currently locked.
    ///
    #[must_use]
    pub fn is_market_locked(&self, target_id: TargetId, current_ns: u64) -> bool {
        self.policy_state.locks.is_locked(target_id, current_ns)
    }
}

// ---------------------------------------------------------------------------
// Soroban Contract Definition
// ---------------------------------------------------------------------------

/// Storage keys for the Soroban vault contract.
#[contracttype]
#[derive(Clone, Debug)]
pub enum VaultDataKey {
    /// Curator address.
    Curator,
    /// Underlying asset token address.
    AssetToken,
    /// Share token address.
    ShareToken,
    /// Fee configuration (borsh-encoded).
    FeesSpec,
    /// Blend adapter contract address.
    BlendAdapter,
    /// Blend pool contract address.
    BlendPool,
    /// Blend factory contract address.
    BlendFactory,
    /// Reentrancy guard flag.
    ReentrancyLock,
    /// Whether the contract is initialized.
    Initialized,
    /// Whether the vault is paused.
    Paused,
}

/// Batched snapshot of vault balances for view calls.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultSnapshot {
    pub total_shares: i128,
    pub idle_assets: i128,
    pub external_assets: i128,
}

/// Soroban vault contract.
///
/// This is the deployable contract that uses Soroban SDK's `#[contract]` macro.
/// It provides the on-chain interface for vault operations.
#[contract]
pub struct SorobanVaultContract;

type ContractVault<'a> = CuratorVault<
    SorobanStorage<'a>,
    RbacAuth,
    SorobanEffectInterpreter<'a, SdkTokenAdapter<'a>, SdkTokenAdapter<'a>>,
    NoopMarketAdapter,
    NoopCrossChainAdapter,
>;

fn extend_storage_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
    let storage = SorobanStorage::new(env);
    storage.extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
}

// ---------------------------------------------------------------------------
// Config address helpers — reduce boilerplate for VaultDataKey get/set
// ---------------------------------------------------------------------------

/// Read a required `SdkAddress` from instance storage, returning
/// `ContractError::MissingConfig` when absent.
pub(crate) fn get_config_address(
    env: &Env,
    key: &VaultDataKey,
) -> Result<SdkAddress, ContractError> {
    env.storage()
        .instance()
        .get(key)
        .ok_or(ContractError::MissingConfig)
}

/// Read an optional `SdkAddress` from instance storage.
fn get_optional_config_address(env: &Env, key: &VaultDataKey) -> Option<SdkAddress> {
    env.storage().instance().get(key)
}

/// Write an `SdkAddress` into instance storage.
pub(crate) fn set_config_address(env: &Env, key: &VaultDataKey, addr: &SdkAddress) {
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

fn query_vault_snapshot(env: &Env) -> VaultSnapshot {
    let storage = SorobanStorage::new(env);
    match storage.load_state() {
        Ok(Some(versioned)) => VaultSnapshot {
            total_shares: to_i128(versioned.state.total_shares).unwrap_or(0),
            idle_assets: to_i128(versioned.state.idle_assets).unwrap_or(0),
            external_assets: to_i128(versioned.state.external_assets).unwrap_or(0),
        },
        Ok(None) | Err(_) => VaultSnapshot {
            total_shares: 0,
            idle_assets: 0,
            external_assets: 0,
        },
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

fn with_contract_vault<T>(
    env: &Env,
    f: impl FnOnce(&mut ContractVault<'_>) -> Result<T, RuntimeError>,
) -> Result<T, RuntimeError> {
    // Block operations during migration (upgrade in progress)
    if stellar_contract_utils::upgradeable::can_complete_migration(env) {
        return Err(RuntimeError::invalid_state(
            "migration in progress - call migrate() first",
        ));
    }

    extend_storage_ttl(env);
    migrate_legacy_paused(env);
    let curator: SdkAddress = get_config_address(env, &VaultDataKey::Curator)
        .map_err(|_| RuntimeError::storage_error("curator not set"))?;
    let asset_token: SdkAddress = get_config_address(env, &VaultDataKey::AssetToken)
        .map_err(|_| RuntimeError::storage_error("asset token not set"))?;
    let share_token: SdkAddress = get_config_address(env, &VaultDataKey::ShareToken)
        .map_err(|_| RuntimeError::storage_error("share token not set"))?;

    let vault_sdk = env.current_contract_address();
    let vault_kernel = kernel_address_from_sdk(env, &vault_sdk);
    let curator_kernel = kernel_address_from_sdk(env, &curator);
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

    if let Some(adapter) = get_optional_config_address(env, &VaultDataKey::BlendAdapter) {
        config = config.with_blend_adapter(kernel_address_from_sdk(env, &adapter));
    }
    if let Some(pool) = get_optional_config_address(env, &VaultDataKey::BlendPool) {
        config = config.with_blend_pool(kernel_address_from_sdk(env, &pool));
    }
    if let Some(factory) = get_optional_config_address(env, &VaultDataKey::BlendFactory) {
        config = config.with_blend_factory(kernel_address_from_sdk(env, &factory));
    }

    let fees = load_fees_spec(env)?;
    config = config.with_fees(fees);

    let storage = SorobanStorage::new(env);
    let paused = storage.is_paused();
    let mut rbac_config = RbacConfig::with_curator(curator_kernel);
    rbac_config.set_paused(paused);
    let auth = RbacAuth::new(rbac_config);

    let share_adapter = SdkTokenAdapter::new(env, &share_token);
    let asset_adapter = SdkTokenAdapter::new(env, &asset_token);
    let interpreter = SorobanEffectInterpreter::new(env, &share_adapter, &asset_adapter);

    let mut vault = CuratorVault::new(
        config,
        storage,
        auth,
        interpreter,
        NoopMarketAdapter,
        NoopCrossChainAdapter,
    );
    vault.load_state()?;

    f(&mut vault)
}

fn with_reentrancy_guard<T>(
    env: &Env,
    f: impl FnOnce() -> Result<T, ContractError>,
) -> Result<T, ContractError> {
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
    /// Initialize the vault contract.
    ///
    /// # Errors
    ///
    /// Returns an error if the contract is already initialized or storage fails.
    pub fn initialize(
        env: Env,
        curator: SdkAddress,
        asset_token: SdkAddress,
        share_token: SdkAddress,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        // Check not already initialized
        if env.storage().instance().has(&VaultDataKey::Initialized) {
            return Err(ContractError::AlreadyInitialized);
        }

        // Store configuration
        set_config_address(&env, &VaultDataKey::Curator, &curator);
        set_config_address(&env, &VaultDataKey::AssetToken, &asset_token);
        set_config_address(&env, &VaultDataKey::ShareToken, &share_token);
        env.storage()
            .instance()
            .set(&VaultDataKey::ReentrancyLock, &false);
        env.storage()
            .instance()
            .set(&VaultDataKey::Initialized, &true);
        store_fees_spec(&env, &FeesSpec::zero()).map_err(ContractError::from)?;

        // Initialize vault state in persistent storage using current version.
        let mut storage = SorobanStorage::new(&env);
        let versioned = VersionedState::new(VaultState::default());
        storage
            .save_state(&versioned)
            .map_err(ContractError::from)?;
        storage.save_paused(false).map_err(ContractError::from)?;
        Ok(())
    }

    /// Deposit assets into the vault with slippage protection.
    ///
    /// Use this over the standard `deposit` when you need `min_shares_out` guarantee.
    pub fn deposit_with_min(
        env: Env,
        owner: SdkAddress,
        receiver: SdkAddress,
        assets: i128,
        min_shares_out: i128,
    ) -> Result<i128, ContractError> {
        // Require authorization from owner
        require_signed(&owner);

        if assets <= 0 {
            return Err(ContractError::InvalidInput);
        }

        let assets_u128 = u128::try_from(assets).map_err(|_| ContractError::ConversionOverflow)?;
        let min_shares_u128 = if min_shares_out < 0 {
            return Err(ContractError::InvalidInput);
        } else {
            u128::try_from(min_shares_out).map_err(|_| ContractError::ConversionOverflow)?
        };
        let now_ns = ledger_timestamp_ns(&env);

        with_reentrancy_guard(&env, || {
            let result = with_contract_vault(&env, |vault| {
                vault.deposit_soroban(
                    &env,
                    owner.clone(),
                    receiver.clone(),
                    assets_u128,
                    min_shares_u128,
                    now_ns,
                )
            })
            .map_err(ContractError::from)?;

            let shares = i128::try_from(result.shares_minted)
                .map_err(|_| ContractError::ConversionOverflow)?;
            Ok(shares)
        })
    }

    /// Request a withdrawal from the vault.
    ///
    pub fn request_withdraw(
        env: Env,
        owner: SdkAddress,
        receiver: SdkAddress,
        shares: i128,
        min_assets_out: i128,
    ) -> Result<u64, ContractError> {
        // Require authorization from owner
        require_signed(&owner);

        if shares <= 0 {
            return Err(ContractError::InvalidInput);
        }
        let shares_u128 = u128::try_from(shares).map_err(|_| ContractError::ConversionOverflow)?;
        let min_assets_u128 = if min_assets_out < 0 {
            return Err(ContractError::InvalidInput);
        } else {
            u128::try_from(min_assets_out).map_err(|_| ContractError::ConversionOverflow)?
        };
        let now_ns = ledger_timestamp_ns(&env);

        with_reentrancy_guard(&env, || {
            let result = with_contract_vault(&env, |vault| {
                vault.request_withdraw_soroban(
                    &env,
                    owner.clone(),
                    receiver.clone(),
                    shares_u128,
                    min_assets_u128,
                    now_ns,
                )
            })
            .map_err(ContractError::from)?;

            Ok(result.request_id)
        })
    }

    /// Execute a pending withdrawal.
    ///
    pub fn execute_withdraw(env: Env, caller: SdkAddress) -> Result<(), ContractError> {
        require_signed(&caller);
        let now_ns = ledger_timestamp_ns(&env);

        with_reentrancy_guard(&env, || {
            with_contract_vault(&env, |vault| {
                vault.execute_withdraw_soroban(&env, caller.clone(), now_ns)
            })
            .map_err(ContractError::from)?;
            Ok(())
        })?;
        Ok(())
    }

    /// Pause or unpause the vault.
    ///
    /// Emits both OZ Pausable events (`Paused`/`Unpaused`) and kernel events
    /// (`PauseUpdated`) for backwards compatibility.
    pub fn set_paused(env: Env, caller: SdkAddress, paused: bool) -> Result<(), ContractError> {
        use stellar_contract_utils::pausable::{emit_paused, emit_unpaused};

        ensure_not_reentrant(&env)?;
        require_signed(&caller);
        let caller_kernel = kernel_address_from_sdk(&env, &caller);

        with_contract_vault(&env, |vault| vault.pause(caller_kernel, paused))
            .map_err(ContractError::from)?;
        env.storage().instance().remove(&VaultDataKey::Paused);

        // Emit OZ Pausable events
        if paused {
            emit_paused(&env);
        } else {
            emit_unpaused(&env);
        }

        // Emit kernel event for backwards compatibility
        let payload =
            borsh::to_vec(&templar_vault_kernel::effects::KernelEvent::PauseUpdated { paused })
                .map_err(|_| RuntimeError::storage_error("failed to serialize pause event"))?;
        crate::effects::KernelEventEnvelope {
            payload: Bytes::from_slice(&env, &payload),
        }
        .publish(&env);
        Ok(())
    }

    /// Set the Blend adapter contract address (curator only).
    pub fn set_blend_adapter(
        env: Env,
        caller: SdkAddress,
        adapter: SdkAddress,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_curator(&env, &caller)?;
        require_contract_address(&adapter, "blend adapter must be a contract address")?;
        set_config_address(&env, &VaultDataKey::BlendAdapter, &adapter);
        Ok(())
    }

    /// Set the Blend pool contract address (curator only).
    pub fn set_blend_pool(
        env: Env,
        caller: SdkAddress,
        pool: SdkAddress,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_curator(&env, &caller)?;
        require_contract_address(&pool, "blend pool must be a contract address")?;
        set_config_address(&env, &VaultDataKey::BlendPool, &pool);
        Ok(())
    }

    /// Set the Blend factory contract address (curator only).
    pub fn set_blend_factory(
        env: Env,
        caller: SdkAddress,
        factory: SdkAddress,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_curator(&env, &caller)?;
        require_contract_address(&factory, "blend factory must be a contract address")?;
        set_config_address(&env, &VaultDataKey::BlendFactory, &factory);
        Ok(())
    }

    /// Register a Soroban address for kernel effect execution (curator only).
    ///
    /// This persists the mapping so later calls can resolve fee recipients or
    /// queued withdrawal addresses without re-providing them.
    pub fn register_address(
        env: Env,
        caller: SdkAddress,
        address: SdkAddress,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_curator(&env, &caller)?;
        with_contract_vault(&env, |vault| {
            vault.register_sdk_address(&env, &address)?;
            Ok(())
        })
        .map_err(ContractError::from)?;
        Ok(())
    }

    /// Get the curator address.
    pub fn curator(env: Env) -> Result<SdkAddress, ContractError> {
        ensure_not_reentrant(&env)?;
        get_config_address(&env, &VaultDataKey::Curator)
    }

    /// Get the asset token address.
    pub fn asset_token(env: Env) -> Result<SdkAddress, ContractError> {
        ensure_not_reentrant(&env)?;
        get_config_address(&env, &VaultDataKey::AssetToken)
    }

    /// Get the share token address.
    pub fn share_token(env: Env) -> Result<SdkAddress, ContractError> {
        ensure_not_reentrant(&env)?;
        get_config_address(&env, &VaultDataKey::ShareToken)
    }

    /// Get the Blend adapter contract address.
    pub fn blend_adapter(env: Env) -> Result<SdkAddress, ContractError> {
        ensure_not_reentrant(&env)?;
        get_config_address(&env, &VaultDataKey::BlendAdapter)
    }

    /// Get the Blend pool contract address.
    pub fn blend_pool(env: Env) -> Result<SdkAddress, ContractError> {
        ensure_not_reentrant(&env)?;
        get_config_address(&env, &VaultDataKey::BlendPool)
    }

    /// Get the Blend factory contract address.
    pub fn blend_factory(env: Env) -> Result<SdkAddress, ContractError> {
        ensure_not_reentrant(&env)?;
        get_config_address(&env, &VaultDataKey::BlendFactory)
    }

    /// Check if the vault is paused.
    pub fn is_paused(env: Env) -> bool {
        must(&env, ensure_not_reentrant(&env));
        let storage = SorobanStorage::new(&env);
        storage.is_paused()
    }

    /// Snapshot of vault balances for efficient off-chain reads.
    pub fn vault_snapshot(env: Env) -> VaultSnapshot {
        must(&env, ensure_not_reentrant(&env));
        query_vault_snapshot(&env)
    }

    /// Get total shares in circulation.
    pub fn total_shares(env: Env) -> i128 {
        must(&env, ensure_not_reentrant(&env));
        query_vault_snapshot(&env).total_shares
    }

    /// Get idle assets (not deployed to markets).
    pub fn idle_assets(env: Env) -> i128 {
        must(&env, ensure_not_reentrant(&env));
        query_vault_snapshot(&env).idle_assets
    }

    /// Get external assets (deployed to markets).
    pub fn external_assets(env: Env) -> i128 {
        must(&env, ensure_not_reentrant(&env));
        query_vault_snapshot(&env).external_assets
    }

    /// Extend the TTL of contract storage.
    ///
    /// Call periodically to prevent state expiry.
    pub fn extend_ttl(env: Env) {
        must(&env, ensure_not_reentrant(&env));
        extend_storage_ttl(&env);
    }

    // -------------------------------------------------------------------------
    // Upgradeable (OZ)
    // -------------------------------------------------------------------------

    /// Upgrade the contract to a new WASM implementation.
    ///
    /// Only the curator can perform upgrades. After calling this, the contract
    /// enters migration mode and normal operations are blocked until `migrate()`
    /// is called.
    ///
    /// # Arguments
    ///
    /// * `new_wasm_hash` - The hash of the new WASM blob uploaded to the ledger.
    /// * `operator` - The curator address performing the upgrade.
    pub fn upgrade(
        env: Env,
        new_wasm_hash: BytesN<32>,
        operator: SdkAddress,
    ) -> Result<(), ContractError> {
        ensure_not_reentrant(&env)?;
        require_curator(&env, &operator)?;

        // Enable migration state before upgrading
        stellar_contract_utils::upgradeable::enable_migration(&env);

        // Replace contract code - takes effect after this invocation completes
        env.deployer().update_current_contract_wasm(new_wasm_hash);

        Ok(())
    }

    /// Complete migration after an upgrade.
    ///
    /// Must be called after `upgrade()` before normal operations can resume.
    /// Runs any pending storage migrations.
    ///
    /// # Arguments
    ///
    /// * `operator` - The curator address completing the migration.
    pub fn migrate(env: Env, operator: SdkAddress) -> Result<(), ContractError> {
        use stellar_contract_utils::upgradeable::{
            complete_migration, ensure_can_complete_migration,
        };

        ensure_not_reentrant(&env)?;
        require_curator(&env, &operator)?;

        // Verify we're in migration state (upgrade was called)
        ensure_can_complete_migration(&env);

        // Run storage migrations
        migrate_legacy_paused(&env);

        // Extend TTL after migration
        extend_storage_ttl(&env);

        // Mark migration as complete - normal operations can resume
        complete_migration(&env);

        Ok(())
    }

    /// Check if migration is in progress.
    ///
    /// Returns true if `upgrade()` was called but `migrate()` has not completed.
    pub fn is_migrating(env: Env) -> bool {
        must(&env, ensure_not_reentrant(&env));
        stellar_contract_utils::upgradeable::can_complete_migration(&env)
    }
}

#[inline]
fn require_signed(addr: &SdkAddress) {
    addr.require_auth();
}

fn require_curator(env: &Env, caller: &SdkAddress) -> Result<(), ContractError> {
    require_signed(caller);
    let curator: SdkAddress = get_config_address(env, &VaultDataKey::Curator)?;
    if caller != &curator {
        return Err(ContractError::Unauthorized);
    }
    Ok(())
}

#[inline]
fn must<T>(env: &Env, result: Result<T, ContractError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic_with_error!(env, err),
    }
}

// ERC-4626 / FungibleVault methods (SEP-56 compatible)
//
// Second #[contractimpl] block exposing the 16 standard FungibleVault
// methods. Must be in the same module as the #[contract] struct to avoid
// Soroban macro conflicts with client generation.

#[contractimpl]
impl SorobanVaultContract {
    /// Returns the address of the underlying asset managed by the vault.
    pub fn query_asset(env: Env) -> SdkAddress {
        must(&env, ensure_not_reentrant(&env));
        must(&env, get_config_address(&env, &VaultDataKey::AssetToken))
    }

    /// Returns the total amount of underlying assets under management.
    ///
    /// Includes both idle assets held in the contract and external assets
    /// deployed to markets.
    pub fn total_assets(env: Env) -> i128 {
        must(&env, ensure_not_reentrant(&env));
        query_vault_field(&env, |s| s.total_assets)
    }

    /// Convert assets to shares (floor rounding, favors vault).
    pub fn convert_to_shares(env: Env, assets: i128) -> i128 {
        must(&env, ensure_not_reentrant(&env));
        if assets <= 0 {
            return 0;
        }
        let (state, config) = must(&env, load_state_and_config(&env));
        let assets_u128 = must(&env, to_u128(assets));
        must(
            &env,
            to_i128(convert_to_shares(&state, &config, assets_u128)),
        )
    }

    /// Convert shares to assets (floor rounding, favors vault).
    pub fn convert_to_assets(env: Env, shares: i128) -> i128 {
        must(&env, ensure_not_reentrant(&env));
        if shares <= 0 {
            return 0;
        }
        let (state, config) = must(&env, load_state_and_config(&env));
        let shares_u128 = must(&env, to_u128(shares));
        must(
            &env,
            to_i128(convert_to_assets(&state, &config, shares_u128)),
        )
    }

    /// Maximum assets that can be deposited for `receiver`.
    ///
    /// Returns the largest safe deposit amount when the vault is idle and
    /// unpaused, 0 otherwise. The cap is constrained to avoid overflow in
    /// kernel accounting and Soroban i128 conversions.
    pub fn max_deposit(env: Env, _receiver: SdkAddress) -> i128 {
        must(&env, ensure_not_reentrant(&env));
        match load_state_and_config(&env) {
            Ok((state, config)) => {
                if state.op_state.is_idle() && !config.paused {
                    let remaining = u128::MAX.saturating_sub(state.total_assets);
                    let cap = remaining.min(i128::MAX as u128);
                    cap as i128
                } else {
                    0
                }
            }
            Err(_) => 0,
        }
    }

    /// Maximum shares that can be minted for `receiver`.
    pub fn max_mint(env: Env, _receiver: SdkAddress) -> i128 {
        must(&env, ensure_not_reentrant(&env));
        match load_state_and_config(&env) {
            Ok((state, config)) => {
                if state.op_state.is_idle() && !config.paused {
                    let remaining = u128::MAX.saturating_sub(state.total_shares);
                    let cap = remaining.min(i128::MAX as u128);
                    cap as i128
                } else {
                    0
                }
            }
            Err(_) => 0,
        }
    }

    /// Maximum assets that `owner` can withdraw atomically.
    ///
    /// Limited by their share balance and available idle assets.
    /// Returns 0 if the vault is not idle.
    pub fn max_withdraw(env: Env, owner: SdkAddress) -> i128 {
        must(&env, ensure_not_reentrant(&env));
        let Ok((state, config)) = load_state_and_config(&env) else {
            return 0;
        };
        if !state.op_state.is_idle() {
            return 0;
        }
        let owner_shares_i128 = share_balance(&env, &owner);
        let owner_shares = owner_shares_i128.max(0) as u128;
        let assets_from_shares = convert_to_assets(&state, &config, owner_shares);
        let max = assets_from_shares.min(state.idle_assets);
        i128::try_from(max).unwrap_or(0)
    }

    /// Maximum shares that `owner` can redeem atomically.
    ///
    /// Limited by their share balance and what idle assets can cover.
    /// Returns 0 if the vault is not idle.
    pub fn max_redeem(env: Env, owner: SdkAddress) -> i128 {
        must(&env, ensure_not_reentrant(&env));
        let Ok((state, config)) = load_state_and_config(&env) else {
            return 0;
        };
        if !state.op_state.is_idle() {
            return 0;
        }
        let owner_shares_i128 = share_balance(&env, &owner);
        let owner_shares = owner_shares_i128.max(0) as u128;
        let shares_from_idle = convert_to_shares(&state, &config, state.idle_assets);
        let max = owner_shares.min(shares_from_idle);
        i128::try_from(max).unwrap_or(0)
    }

    /// Preview shares received for a deposit of `assets` (floor — fewer shares).
    pub fn preview_deposit(env: Env, assets: i128) -> i128 {
        Self::convert_to_shares(env, assets)
    }

    /// Preview assets needed to mint `shares` (ceil — more assets required).
    pub fn preview_mint(env: Env, shares: i128) -> i128 {
        must(&env, ensure_not_reentrant(&env));
        if shares <= 0 {
            return 0;
        }
        let (state, config) = must(&env, load_state_and_config(&env));
        let shares_u128 = must(&env, to_u128(shares));
        must(
            &env,
            to_i128(convert_to_assets_ceil(&state, &config, shares_u128)),
        )
    }

    /// Preview shares burned to withdraw `assets` (ceil — more shares burned).
    pub fn preview_withdraw(env: Env, assets: i128) -> i128 {
        must(&env, ensure_not_reentrant(&env));
        if assets <= 0 {
            return 0;
        }
        let (state, config) = must(&env, load_state_and_config(&env));
        let assets_u128 = must(&env, to_u128(assets));
        must(
            &env,
            to_i128(convert_to_shares_ceil(&state, &config, assets_u128)),
        )
    }

    /// Preview assets received for redeeming `shares` (floor — fewer assets).
    pub fn preview_redeem(env: Env, shares: i128) -> i128 {
        Self::convert_to_assets(env, shares)
    }

    /// Deposit `assets` and mint shares to `receiver`. Returns shares minted.
    ///
    /// The `operator` must have authorized the call. `from` provides the assets.
    /// The vault must be idle and unpaused.
    pub fn deposit(
        env: Env,
        assets: i128,
        receiver: SdkAddress,
        from: SdkAddress,
        operator: SdkAddress,
    ) -> i128 {
        require_signed(&operator);
        must(&env, ensure_not_reentrant(&env));
        if assets <= 0 {
            panic_with_error!(&env, ContractError::InvalidInput);
        }
        must(
            &env,
            Self::deposit_with_min(env.clone(), from, receiver, assets, 0),
        )
    }

    /// Mint exactly `shares` to `receiver`, pulling required assets from `from`.
    /// Returns assets deposited.
    pub fn mint(
        env: Env,
        shares: i128,
        receiver: SdkAddress,
        from: SdkAddress,
        operator: SdkAddress,
    ) -> i128 {
        require_signed(&operator);
        must(&env, ensure_not_reentrant(&env));
        if shares <= 0 {
            panic_with_error!(&env, ContractError::InvalidInput);
        }
        let (state, config) = must(&env, load_state_and_config(&env));
        let shares_u128 = must(&env, to_u128(shares));
        let assets_needed = convert_to_assets_ceil(&state, &config, shares_u128);
        let assets_i128 = must(&env, to_i128(assets_needed));
        let _shares_minted = must(
            &env,
            Self::deposit_with_min(env.clone(), from, receiver, assets_i128, shares),
        );
        assets_i128
    }

    /// Withdraw exactly `assets` from the vault, burning shares from `owner`.
    /// Returns shares burned.
    ///
    /// Only works when the vault is Idle and has sufficient idle assets.
    /// For the general case, use `request_withdraw` + `execute_withdraw`.
    pub fn withdraw(
        env: Env,
        assets: i128,
        receiver: SdkAddress,
        owner: SdkAddress,
        operator: SdkAddress,
    ) -> i128 {
        require_signed(&operator);
        require_signed(&owner);
        if assets <= 0 {
            panic_with_error!(&env, ContractError::InvalidInput);
        }
        must(
            &env,
            with_reentrancy_guard(&env, || {
                let assets_u128 = must(&env, to_u128(assets));
                let (state, _config) = must(&env, load_state_and_config(&env));
                if !state.op_state.is_idle() {
                    panic_with_error!(&env, ContractError::VaultNotIdle);
                }
                if assets_u128 > state.idle_assets {
                    panic_with_error!(&env, ContractError::InsufficientIdleAssets);
                }
                must(&env, refresh_fees_for_atomic(&env));
                let (state, config) = must(&env, load_state_and_config(&env));
                let shares_to_burn = convert_to_shares_ceil(&state, &config, assets_u128);
                must(
                    &env,
                    atomic_withdraw_internal(&env, &owner, &receiver, assets_u128, shares_to_burn),
                );
                Ok(must(&env, to_i128(shares_to_burn)))
            }),
        )
    }

    /// Redeem exactly `shares` for assets, sending to `receiver`.
    /// Returns assets received.
    ///
    /// Only works when the vault is Idle and has sufficient idle assets.
    /// For the general case, use `request_withdraw` + `execute_withdraw`.
    pub fn redeem(
        env: Env,
        shares: i128,
        receiver: SdkAddress,
        owner: SdkAddress,
        operator: SdkAddress,
    ) -> i128 {
        require_signed(&operator);
        require_signed(&owner);
        if shares <= 0 {
            panic_with_error!(&env, ContractError::InvalidInput);
        }
        must(
            &env,
            with_reentrancy_guard(&env, || {
                let shares_u128 = must(&env, to_u128(shares));
                let (state, _config) = must(&env, load_state_and_config(&env));
                if !state.op_state.is_idle() {
                    panic_with_error!(&env, ContractError::VaultNotIdle);
                }
                must(&env, refresh_fees_for_atomic(&env));
                let (state, config) = must(&env, load_state_and_config(&env));
                let assets_out = convert_to_assets(&state, &config, shares_u128);
                if assets_out > state.idle_assets {
                    panic_with_error!(&env, ContractError::InsufficientIdleAssets);
                }
                must(
                    &env,
                    atomic_withdraw_internal(&env, &owner, &receiver, assets_out, shares_u128),
                );
                Ok(must(&env, to_i128(assets_out)))
            }),
        )
    }
}

#[cfg(test)]
mod tests;
