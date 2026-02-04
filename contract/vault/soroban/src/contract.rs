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

use alloc::vec;
use alloc::vec::Vec;
use soroban_sdk::{
    contract, contractimpl, contracttype, Address as SdkAddress, Bytes, Env,
};
use templar_curator_primitives::{
    determine_recovery_action, PolicyState, RecoveryContext, RecoveryProgress,
};
use templar_vault_kernel::{
    apply_action, complete_allocation, complete_refresh, compute_fee_shares_from_assets,
    mul_div_floor, preview_deposit_shares, preview_withdraw_assets, start_allocation,
    start_refresh, withdrawal_collected, withdrawal_step_callback, Address, FeeAccrualAnchor,
    FeesSpec, KernelAction, Number, OpState, PayoutOutcome, Restrictions, TargetId, VaultConfig,
    VaultState, MAX_PENDING, MIN_WITHDRAWAL_ASSETS,
};
use templar_vault_kernel::effects::{KernelEffect, KernelEvent};
use templar_vault_kernel::state::queue::{
    can_partially_satisfy, compute_full_withdrawal, compute_partial_withdrawal,
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
use crate::rbac::{RbacAuth, RbacConfig};
use crate::storage::{SorobanStorage, Storage, VersionedState};

const ESCROW_ADDRESS: Address = [0u8; 32];
const KERNEL_ADDRESS_DOMAIN: &[u8] = b"templar:soroban:address";
const YEAR_NS: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;
const TTL_THRESHOLD_LEDGERS: u32 = 50_000;
const TTL_EXTEND_TO_LEDGERS: u32 = 100_000;

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

#[derive(Clone, Copy, Debug, Default)]
struct NoopMarketAdapter;

impl MarketAdapter for NoopMarketAdapter {
    fn supply(&mut self, _market: MarketRef, _amount: u128) -> Result<(), RuntimeError> {
        Err(RuntimeError::contract_error("market adapter not configured"))
    }

    fn withdraw(&mut self, _market: MarketRef, _amount: u128) -> Result<(), RuntimeError> {
        Err(RuntimeError::contract_error("market adapter not configured"))
    }

    fn total_assets(&self, _market: MarketRef) -> Result<u128, RuntimeError> {
        Err(RuntimeError::contract_error("market adapter not configured"))
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct NoopCrossChainAdapter;

impl CrossChainMarketAdapter for NoopCrossChainAdapter {
    fn submit_intent(&mut self, _plan_bytes: Vec<u8>) -> Result<crate::market::AttemptId, RuntimeError> {
        Err(RuntimeError::contract_error("cross-chain adapter not configured"))
    }

    fn settle(
        &mut self,
        _op_id: u64,
        _attempt_id: crate::market::AttemptId,
    ) -> Result<crate::market::SettlementReceipt, RuntimeError> {
        Err(RuntimeError::contract_error("cross-chain adapter not configured"))
    }

    fn total_assets(&self, _market: MarketRef) -> Result<u128, RuntimeError> {
        Err(RuntimeError::contract_error("cross-chain adapter not configured"))
    }
}

fn serialize_fees_spec(fees: &FeesSpec) -> Result<Vec<u8>, RuntimeError> {
    borsh::to_vec(fees).map_err(|_| RuntimeError::storage_error("fees serialize failed"))
}

fn deserialize_fees_spec(bytes: &[u8]) -> Result<FeesSpec, RuntimeError> {
    <FeesSpec as borsh::BorshDeserialize>::try_from_slice(bytes)
        .map_err(|_| RuntimeError::storage_error("fees deserialize failed"))
}

fn load_fees_spec(env: &Env) -> Result<FeesSpec, RuntimeError> {
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
    /// Administrator address.
    pub admin: Address,
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
        admin: Address,
        vault_address: Address,
        guardians: Vec<Address>,
        allocators: Vec<Address>,
        asset_address: Address,
        share_address: Address,
    ) -> Self {
        Self {
            admin,
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

    /// Check if the given address is the admin.
    #[inline]
    #[must_use]
    pub fn is_admin(&self, addr: &Address) -> bool {
        &self.admin == addr
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

    /// Check if the address has privileged access (admin or allocator).
    #[inline]
    #[must_use]
    pub fn is_privileged(&self, addr: &Address) -> bool {
        self.is_admin(addr) || self.is_allocator(addr)
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
    pub fn register_address(&mut self, kernel_addr: Address, soroban_addr: SdkAddress) {
        self.interpreter
            .register_address(kernel_addr, soroban_addr);
    }

    /// Save vault state to storage.
    pub fn save_state(&mut self) -> Result<(), RuntimeError> {
        if let Some(ref state) = self.state {
            let versioned = VersionedState::new(state.clone());
            self.storage.save_state(&versioned)?;
        }
        Ok(())
    }

    /// Get a reference to the current vault state.
    ///
    /// # Panics
    ///
    /// Panics if state has not been loaded.
    #[inline]
    #[must_use]
    pub fn state(&self) -> &VaultState {
        self.state.as_ref().expect("state not loaded")
    }

    /// Get a mutable reference to the current vault state.
    ///
    /// # Panics
    ///
    /// Panics if state has not been loaded.
    #[inline]
    pub fn state_mut(&mut self) -> &mut VaultState {
        self.state.as_mut().expect("state not loaded")
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
        self.interpreter
            .register_address(vault_kernel, vault_sdk.clone());
        self.interpreter
            .register_address(ESCROW_ADDRESS, vault_sdk);
        Ok(())
    }

    fn register_sdk_address(&mut self, env: &Env, addr: &SdkAddress) -> Address {
        let kernel_addr = kernel_address_from_sdk(env, addr);
        self.interpreter
            .register_address(kernel_addr, addr.clone());
        kernel_addr
    }

    fn kernel_config(&self) -> VaultConfig {
        VaultConfig {
            fees: self.config.fees,
            min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
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
        let state = self.state().clone();
        let result = apply_action(state, &config, restrictions, &self.config.vault_address, action)
            .map_err(RuntimeError::transition_error)?;

        let ctx = self.effect_context(now_ns);
        self.ensure_effect_addresses_mapped(&result.effects, &ctx)?;
        let summary = self
            .interpreter
            .execute_effects(&result.effects, &ctx)?;

        self.state = Some(result.state);
        self.save_state()?;

        Ok(summary)
    }

    fn ensure_effect_addresses_mapped(
        &self,
        effects: &[KernelEffect],
        ctx: &EffectContext,
    ) -> Result<(), RuntimeError> {
        for effect in effects {
            match effect {
                KernelEffect::MintShares { owner, .. }
                | KernelEffect::BurnShares { owner, .. } => {
                    self.require_mapped(owner)?;
                }
                KernelEffect::TransferShares { from, to, .. } => {
                    self.require_mapped(from)?;
                    self.require_mapped(to)?;
                }
                KernelEffect::TransferAssets { to, .. } => {
                    self.require_mapped(&ctx.vault_address)?;
                    self.require_mapped(to)?;
                }
                KernelEffect::TransferAssetsFrom { from, to, .. } => {
                    self.require_mapped(from)?;
                    self.require_mapped(to)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn require_mapped(&self, addr: &Address) -> Result<(), RuntimeError> {
        if self.interpreter.has_address(addr) {
            Ok(())
        } else {
            Err(RuntimeError::effect_failed("missing address mapping"))
        }
    }

    // =========================================================================
    // User-facing entrypoints
    // =========================================================================

    /// Deposit assets into the vault.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address
    /// * `receiver` - The address to receive shares
    /// * `assets` - Amount of assets to deposit
    /// * `min_shares_out` - Minimum shares expected (slippage protection)
    /// * `now_ns` - Current timestamp in nanoseconds
    ///
    /// # Returns
    ///
    /// `Ok(DepositResult)` on success
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

        Ok(DepositResult {
            shares_minted: shares,
            total_shares: self.state().total_shares,
            total_assets: self.state().total_assets,
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
        let caller_kernel = self.register_sdk_address(env, &caller);
        let receiver_kernel = self.register_sdk_address(env, &receiver);
        self.deposit(caller_kernel, receiver_kernel, assets, min_shares_out, now_ns)
    }

    /// Request a withdrawal from the vault.
    ///
    /// This queues a withdrawal request. The actual withdrawal will be processed
    /// when `execute_withdraw` is called.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (owner of shares)
    /// * `receiver` - The address to receive assets
    /// * `shares` - Amount of shares to redeem
    /// * `min_assets_out` - Minimum assets expected (slippage protection)
    /// * `now_ns` - Current timestamp in nanoseconds
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

        if self.state().total_shares == 0 {
            return Err(RuntimeError::contract_error("no shares in vault"));
        }

        let request_id = self
            .state()
            .withdraw_queue
            .next_pending_withdrawal_id;

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
        let caller_kernel = self.register_sdk_address(env, &caller);
        let receiver_kernel = self.register_sdk_address(env, &receiver);
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
    /// # Arguments
    ///
    /// * `caller` - The caller's address
    /// * `now_ns` - Current timestamp in nanoseconds
    pub fn execute_withdraw(
        &mut self,
        caller: Address,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::ExecuteWithdraw, caller, None)?;

        let mut summary = EffectSummary::new();

        if self.state().op_state.is_idle() {
            let step_summary = self.apply_kernel_action(
                KernelAction::ExecuteWithdraw { now_ns },
                now_ns,
            )?;
            summary.merge(step_summary);
        } else if !self.state().op_state.is_withdrawing() {
            return Err(RuntimeError::contract_error(
                "vault not in idle or withdrawing state for withdrawal",
            ));
        }

        if self.state().op_state.is_withdrawing() {
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
        let caller_kernel = self.register_sdk_address(env, &caller);
        self.execute_withdraw(caller_kernel, now_ns)
    }

    fn complete_withdrawal_from_idle(&mut self, now_ns: u64) -> Result<EffectSummary, RuntimeError> {
        let (_, pending) = self
            .state()
            .withdraw_queue
            .head()
            .ok_or_else(|| RuntimeError::contract_error("withdraw queue empty"))?;

        let withdraw = match &self.state().op_state {
            OpState::Withdrawing(state) => state,
            _ => {
                return Err(RuntimeError::contract_error(
                    "withdrawal not in progress",
                ))
            }
        };

        if pending.owner != withdraw.owner
            || pending.receiver != withdraw.receiver
            || pending.escrow_shares != withdraw.escrow_shares
        {
            return Err(RuntimeError::contract_error(
                "withdrawal queue head mismatch",
            ));
        }

        let available_assets = self.state().idle_assets;
        if available_assets == 0 {
            return Ok(EffectSummary::new());
        }

        let withdrawal_result = if available_assets >= pending.expected_assets {
            compute_full_withdrawal(pending, available_assets)
                .ok_or_else(|| RuntimeError::contract_error("withdrawal not satisfiable"))?
        } else {
            if !can_partially_satisfy(pending, available_assets) {
                return Ok(EffectSummary::new());
            }
            compute_partial_withdrawal(pending, available_assets)
        };

        let assets_out = withdrawal_result.assets_out;
        if assets_out == 0 {
            return Ok(EffectSummary::new());
        }

        let burn_shares = withdrawal_result.settlement.to_burn;
        let refund_shares = withdrawal_result.settlement.refund;
        let op_id = withdraw.op_id;

        let step = withdrawal_step_callback(self.state().op_state.clone(), op_id, assets_out)
            .map_err(RuntimeError::transition_error)?;
        self.state_mut().op_state = step.new_state;

        let collected = withdrawal_collected(self.state().op_state.clone(), op_id, burn_shares)
            .map_err(RuntimeError::transition_error)?;
        let ctx = self.effect_context(now_ns);
        let mut summary = self.interpreter.execute_effects(&collected.effects, &ctx)?;
        self.state_mut().op_state = collected.new_state;

        let payout = match &self.state().op_state {
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
        let transfer_summary = self
            .interpreter
            .execute_effects(&transfer_effects, &ctx)?;
        summary.merge(transfer_summary);

        let state = self.state_mut();
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
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be guardian or admin)
    /// * `paused` - Whether to pause (true) or unpause (false)
    pub fn pause(&mut self, caller: Address, paused: bool) -> Result<(), RuntimeError> {
        // Authorize
        self.auth.authorize(ActionKind::Pause, caller, None)?;

        self.paused = paused;
        self.storage.save_paused(paused)?;
        Ok(())
    }

    /// Set kernel restrictions for the vault.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be admin)
    /// * `restrictions` - Optional restrictions policy
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
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `plan` - Allocation plan: list of (target_id, amount) pairs
    /// * `current_ns` - Current timestamp in nanoseconds (for lock expiry checks)
    pub fn begin_allocating(
        &mut self,
        caller: Address,
        plan: Vec<(TargetId, u128)>,
        current_ns: u64,
    ) -> Result<u64, RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::BeginAllocating, caller, None)?;

        // Filter plan to exclude locked markets
        let filtered_plan =
            filter_allocation_plan(&plan, &self.policy_state.locks, current_ns);

        let state = self.state_mut();
        let op_id = state.next_op_id;
        state.next_op_id = state.next_op_id.saturating_add(1);

        // Call kernel transition with filtered plan
        let result = start_allocation(state.op_state.clone(), filtered_plan, op_id)
            .map_err(RuntimeError::transition_error)?;

        state.op_state = result.new_state;
        self.save_state()?;

        Ok(op_id)
    }

    /// Sync external assets during an operation.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `new_external_assets` - Updated external assets value
    /// * `op_id` - Operation ID to verify correlation
    /// * `now_ns` - Current timestamp in nanoseconds
    pub fn sync_external_assets(
        &mut self,
        caller: Address,
        new_external_assets: u128,
        op_id: u64,
        now_ns: u64,
    ) -> Result<(), RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::SyncExternalAssets, caller, None)?;

        let action = KernelAction::SyncExternalAssets {
            new_external_assets,
            op_id,
            now_ns,
        };
        let _summary = self.apply_kernel_action(action, now_ns)?;

        Ok(())
    }

    /// Finish an allocation operation.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `op_id` - Operation ID to verify correlation
    pub fn finish_allocating(
        &mut self,
        caller: Address,
        op_id: u64,
    ) -> Result<AllocationResult, RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::FinishAllocating, caller, None)?;

        // Call kernel transition
        {
            let state = self.state_mut();
            let transition_result = complete_allocation(state.op_state.clone(), op_id, None)
                .map_err(RuntimeError::transition_error)?;
            state.op_state = transition_result.new_state;
        }

        // Capture external_assets before save_state
        let external_assets = self.state().external_assets;
        self.save_state()?;

        Ok(AllocationResult {
            op_id,
            new_external_assets: external_assets,
            summary: EffectSummary::new(),
        })
    }

    /// Begin a refresh operation.
    ///
    /// Filters the plan to exclude locked markets before starting.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `plan` - List of target IDs to refresh
    /// * `current_ns` - Current timestamp in nanoseconds (for lock expiry checks)
    pub fn begin_refreshing(
        &mut self,
        caller: Address,
        plan: Vec<TargetId>,
        current_ns: u64,
    ) -> Result<u64, RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::BeginRefreshing, caller, None)?;

        // Filter plan to exclude locked markets
        let filtered_plan =
            build_refresh_plan_with_locks(&plan, &self.policy_state.locks, current_ns);

        let state = self.state_mut();
        let op_id = state.next_op_id;
        state.next_op_id = state.next_op_id.saturating_add(1);

        // Call kernel transition with filtered plan
        let result = start_refresh(state.op_state.clone(), filtered_plan, op_id)
            .map_err(RuntimeError::transition_error)?;

        state.op_state = result.new_state;
        self.save_state()?;

        Ok(op_id)
    }

    /// Finish a refresh operation.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `op_id` - Operation ID to verify correlation
    pub fn finish_refreshing(
        &mut self,
        caller: Address,
        op_id: u64,
    ) -> Result<RefreshResult, RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::FinishRefreshing, caller, None)?;

        let state = self.state_mut();
        let markets_refreshed = match &state.op_state {
            OpState::Refreshing(refresh) => refresh.plan.len() as u32,
            _ => 0,
        };

        // Call kernel transition
        let result = complete_refresh(state.op_state.clone(), op_id)
            .map_err(RuntimeError::transition_error)?;

        state.op_state = result.new_state;
        let external_assets = state.external_assets;
        self.save_state()?;

        Ok(RefreshResult {
            op_id,
            markets_refreshed,
            new_external_assets: external_assets,
        })
    }

    /// Abort an allocation operation.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `op_id` - Operation ID to verify correlation
    /// * `restore_idle` - Amount of idle assets to restore
    pub fn abort_allocating(
        &mut self,
        caller: Address,
        op_id: u64,
        restore_idle: u128,
    ) -> Result<(), RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::AbortAllocating, caller, None)?;
        let action = KernelAction::AbortAllocating {
            op_id,
            restore_idle,
        };
        let _summary = self.apply_kernel_action(action, 0)?;
        Ok(())
    }

    /// Abort a refresh operation.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `op_id` - Operation ID to verify correlation
    pub fn abort_refreshing(&mut self, caller: Address, op_id: u64) -> Result<(), RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::AbortRefreshing, caller, None)?;
        let action = KernelAction::AbortRefreshing { op_id };
        let _summary = self.apply_kernel_action(action, 0)?;
        Ok(())
    }

    /// Abort a withdrawal operation.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `op_id` - Operation ID to verify correlation
    /// * `refund_shares` - Shares to refund to the owner
    pub fn abort_withdrawing(
        &mut self,
        caller: Address,
        op_id: u64,
        refund_shares: u128,
    ) -> Result<(), RuntimeError> {
        self.auth
            .authorize(ActionKind::AbortWithdrawing, caller, None)?;
        let action = KernelAction::AbortWithdrawing {
            op_id,
            refund_shares,
        };
        let _summary = self.apply_kernel_action(action, 0)?;
        Ok(())
    }

    /// Settle a payout operation.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `op_id` - Operation ID to verify correlation
    /// * `outcome` - Payout outcome (success or failure)
    pub fn settle_payout(
        &mut self,
        caller: Address,
        op_id: u64,
        outcome: PayoutOutcome,
    ) -> Result<(), RuntimeError> {
        self.auth
            .authorize(ActionKind::SettlePayout, caller, None)?;
        let action = KernelAction::SettlePayout { op_id, outcome };
        let _summary = self.apply_kernel_action(action, 0)?;
        Ok(())
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
        let Some(action) =
            determine_recovery_action(&self.state().op_state, &context, &progress)
        else {
            return Ok(None);
        };

        let kind: ActionKind = (&action).into();
        self.auth.authorize(kind, caller, None)?;

        let summary = self.apply_kernel_action(action, context.current_ns)?;
        Ok(Some(summary))
    }

    /// Manual reconciliation of external assets.
    ///
    /// This is a privileged entrypoint that runs a full refresh cycle in one call:
    /// BeginRefreshing -> read principals -> SyncExternalAssets -> FinishRefreshing
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be admin or guardian)
    /// * `markets` - Market references to reconcile
    /// * `now_ns` - Current timestamp
    pub fn manual_reconcile(
        &mut self,
        caller: Address,
        markets: Vec<MarketRef>,
        now_ns: u64,
    ) -> Result<ReconciliationRecord, RuntimeError> {
        // Authorize - requires ManualReconcile privilege
        self.auth
            .authorize(ActionKind::ManualReconcile, caller, None)?;

        // Phase 1: Check state and start refresh
        let op_id = {
            let state = self.state_mut();

            // Ensure we're in Idle state
            if !state.op_state.is_idle() {
                return Err(RuntimeError::contract_error("vault not in idle state"));
            }

            // Generate op_id
            let op_id = state.next_op_id;
            state.next_op_id = state.next_op_id.saturating_add(1);

            op_id
        };

        // Build plan from markets and filter locked ones
        let plan: Vec<TargetId> = markets.iter().map(|m| m.market_id).collect();
        let filtered_plan =
            build_refresh_plan_with_locks(&plan, &self.policy_state.locks, now_ns);

        // Start refresh with filtered plan
        {
            let state = self.state_mut();
            let result = start_refresh(state.op_state.clone(), filtered_plan, op_id)
                .map_err(RuntimeError::transition_error)?;
            state.op_state = result.new_state;
        }

        // Phase 2: Reconcile external assets (releases mutable borrow)
        let record = reconcile_external_assets(&self.market, op_id, &markets)?;

        // Phase 3: Update state with reconciliation results
        {
            let state = self.state_mut();

            // Update external assets
            let old_external = state.external_assets;
            state.external_assets = record.new_external_assets;

            // Adjust total_assets
            if record.new_external_assets > old_external {
                let increase = record.new_external_assets - old_external;
                state.total_assets = state.total_assets.saturating_add(increase);
            } else {
                let decrease = old_external - record.new_external_assets;
                state.total_assets = state.total_assets.saturating_sub(decrease);
            }

            // Complete refresh
            let result = complete_refresh(state.op_state.clone(), op_id)
                .map_err(RuntimeError::transition_error)?;
            state.op_state = result.new_state;
        }

        // Phase 4: Emit audit event
        let ctx = self.effect_context(now_ns);
        let effect = templar_vault_kernel::effects::KernelEffect::EmitEvent {
            event: templar_vault_kernel::effects::KernelEvent::RefreshCompleted { op_id },
        };
        self.interpreter.execute_effect(&effect, &ctx)?;

        self.save_state()?;

        Ok(record)
    }

    /// Refresh fees based on elapsed time.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address
    /// * `now_ns` - Current timestamp in nanoseconds
    ///
    /// # Returns
    ///
    /// Total shares minted for fees.
    pub fn refresh_fees(&mut self, caller: Address, now_ns: u64) -> Result<u128, RuntimeError> {
        // Authorize
        self.auth.authorize(ActionKind::RefreshFees, caller, None)?;

        let state = self.state().clone();
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

    /// Acquire a market lock.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `target_id` - Market to lock
    /// * `expiry_ns` - Lock expiry timestamp in nanoseconds
    /// * `current_ns` - Current timestamp
    pub fn acquire_market_lock(
        &mut self,
        caller: Address,
        target_id: TargetId,
        expiry_ns: u64,
        current_ns: u64,
    ) -> Result<(), RuntimeError> {
        use crate::policy::MarketLock;

        // Authorize - requires allocator privileges
        self.auth
            .authorize(ActionKind::BeginAllocating, caller, None)?;

        let lock = MarketLock::new(target_id, current_ns).with_expiry(expiry_ns);
        self.policy_state.locks =
            self.policy_state.locks.acquire(lock, current_ns).map_err(|e| {
                RuntimeError::contract_error(alloc::format!("failed to acquire lock: {:?}", e))
            })?;
        self.storage.save_policy_state(&self.policy_state)?;

        Ok(())
    }

    /// Release a market lock.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `target_id` - Market to unlock
    pub fn release_market_lock(
        &mut self,
        caller: Address,
        target_id: TargetId,
    ) -> Result<(), RuntimeError> {
        // Authorize - requires allocator privileges
        self.auth
            .authorize(ActionKind::BeginAllocating, caller, None)?;

        self.policy_state.locks = self.policy_state.locks.release(target_id);
        self.storage.save_policy_state(&self.policy_state)?;

        Ok(())
    }

    /// Check if a market is currently locked.
    ///
    /// # Arguments
    ///
    /// * `target_id` - Market to check
    /// * `current_ns` - Current timestamp
    ///
    /// # Returns
    ///
    /// `true` if the market is locked, `false` otherwise.
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
    /// Admin address.
    Admin,
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
        .extend_ttl(TTL_THRESHOLD_LEDGERS, TTL_EXTEND_TO_LEDGERS);
    let storage = SorobanStorage::new(env);
    storage.extend_ttl(TTL_THRESHOLD_LEDGERS, TTL_EXTEND_TO_LEDGERS);
}

fn with_contract_vault<T>(
    env: &Env,
    f: impl FnOnce(&mut ContractVault<'_>) -> Result<T, RuntimeError>,
) -> Result<T, RuntimeError> {
    extend_storage_ttl(env);
    let admin: SdkAddress = env
        .storage()
        .instance()
        .get(&VaultDataKey::Admin)
        .ok_or_else(|| RuntimeError::storage_error("admin not set"))?;
    let asset_token: SdkAddress = env
        .storage()
        .instance()
        .get(&VaultDataKey::AssetToken)
        .ok_or_else(|| RuntimeError::storage_error("asset token not set"))?;
    let share_token: SdkAddress = env
        .storage()
        .instance()
        .get(&VaultDataKey::ShareToken)
        .ok_or_else(|| RuntimeError::storage_error("share token not set"))?;

    let vault_sdk = env.current_contract_address();
    let vault_kernel = kernel_address_from_sdk(env, &vault_sdk);
    let admin_kernel = kernel_address_from_sdk(env, &admin);
    let asset_kernel = kernel_address_from_sdk(env, &asset_token);
    let share_kernel = kernel_address_from_sdk(env, &share_token);

    let mut config = ContractConfig::new(
        admin_kernel,
        vault_kernel,
        Vec::new(),
        Vec::new(),
        asset_kernel,
        share_kernel,
    );

    if let Some(adapter) = env.storage().instance().get(&VaultDataKey::BlendAdapter) {
        config = config.with_blend_adapter(kernel_address_from_sdk(env, &adapter));
    }
    if let Some(pool) = env.storage().instance().get(&VaultDataKey::BlendPool) {
        config = config.with_blend_pool(kernel_address_from_sdk(env, &pool));
    }
    if let Some(factory) = env.storage().instance().get(&VaultDataKey::BlendFactory) {
        config = config.with_blend_factory(kernel_address_from_sdk(env, &factory));
    }

    let fees = load_fees_spec(env)?;
    config = config.with_fees(fees);

    let storage = SorobanStorage::new(env);
    let paused = storage.is_paused();
    let mut rbac_config = RbacConfig::with_admin(admin_kernel);
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
    let locked: bool = env
        .storage()
        .instance()
        .get(&VaultDataKey::ReentrancyLock)
        .unwrap_or(false);
    if locked {
        return Err(ContractError::Reentrancy);
    }
    env.storage()
        .instance()
        .set(&VaultDataKey::ReentrancyLock, &true);
    let result = f();
    env.storage()
        .instance()
        .set(&VaultDataKey::ReentrancyLock, &false);
    result
}

#[contractimpl]
impl SorobanVaultContract {
    /// Initialize the vault contract.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment
    /// * `admin` - Administrator address
    /// * `asset_token` - Address of the underlying asset token
    /// * `share_token` - Address of the share token
    ///
    /// # Errors
    ///
    /// Returns an error if the contract is already initialized or storage fails.
    pub fn initialize(
        env: Env,
        admin: SdkAddress,
        asset_token: SdkAddress,
        share_token: SdkAddress,
    ) -> Result<(), ContractError> {
        // Check not already initialized
        if env
            .storage()
            .instance()
            .has(&VaultDataKey::Initialized)
        {
            return Err(ContractError::AlreadyInitialized);
        }

        // Store configuration
        env.storage()
            .instance()
            .set(&VaultDataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&VaultDataKey::AssetToken, &asset_token);
        env.storage()
            .instance()
            .set(&VaultDataKey::ShareToken, &share_token);
        env.storage()
            .instance()
            .set(&VaultDataKey::Paused, &false);
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

    /// Deposit assets into the vault.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment
    /// * `owner` - The depositor's address
    /// * `receiver` - The address to receive shares
    /// * `assets` - Amount of assets to deposit
    /// * `min_shares_out` - Minimum shares expected (slippage protection)
    ///
    /// # Returns
    ///
    /// The number of shares minted.
    pub fn deposit(
        env: Env,
        owner: SdkAddress,
        receiver: SdkAddress,
        assets: i128,
        min_shares_out: i128,
    ) -> Result<i128, ContractError> {
        // Require authorization from owner
        owner.require_auth();

        if assets <= 0 {
            return Err(ContractError::InvalidInput);
        }

        let assets_u128 =
            u128::try_from(assets).map_err(|_| ContractError::ConversionOverflow)?;
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
    /// # Arguments
    ///
    /// * `env` - The Soroban environment
    /// * `owner` - The share owner's address
    /// * `receiver` - The address to receive assets
    /// * `shares` - Amount of shares to redeem
    /// * `min_assets_out` - Minimum assets expected (slippage protection)
    ///
    /// # Returns
    ///
    /// The withdrawal request ID.
    pub fn request_withdraw(
        env: Env,
        owner: SdkAddress,
        receiver: SdkAddress,
        shares: i128,
        min_assets_out: i128,
    ) -> Result<u64, ContractError> {
        // Require authorization from owner
        owner.require_auth();

        if shares <= 0 {
            return Err(ContractError::InvalidInput);
        }
        let shares_u128 =
            u128::try_from(shares).map_err(|_| ContractError::ConversionOverflow)?;
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
    /// # Arguments
    ///
    /// * `env` - The Soroban environment
    /// * `caller` - The caller's address
    pub fn execute_withdraw(env: Env, caller: SdkAddress) -> Result<(), ContractError> {
        caller.require_auth();
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
    /// # Arguments
    ///
    /// * `env` - The Soroban environment
    /// * `caller` - The caller (must be admin)
    /// * `paused` - Whether to pause (true) or unpause (false)
    pub fn set_paused(
        env: Env,
        caller: SdkAddress,
        paused: bool,
    ) -> Result<(), ContractError> {
        caller.require_auth();
        let caller_kernel = kernel_address_from_sdk(&env, &caller);

        with_contract_vault(&env, |vault| vault.pause(caller_kernel, paused))
            .map_err(ContractError::from)?;

        // Keep legacy paused flag in sync.
        env.storage()
            .instance()
            .set(&VaultDataKey::Paused, &paused);

        // Emit event
        use crate::effects::PauseUpdatedEvent;
        PauseUpdatedEvent { paused }.publish(&env);
        Ok(())
    }

    /// Set the Blend adapter contract address (admin only).
    pub fn set_blend_adapter(
        env: Env,
        caller: SdkAddress,
        adapter: SdkAddress,
    ) -> Result<(), ContractError> {
        require_admin(&env, &caller)?;
        env.storage()
            .instance()
            .set(&VaultDataKey::BlendAdapter, &adapter);
        Ok(())
    }

    /// Set the Blend pool contract address (admin only).
    pub fn set_blend_pool(
        env: Env,
        caller: SdkAddress,
        pool: SdkAddress,
    ) -> Result<(), ContractError> {
        require_admin(&env, &caller)?;
        env.storage()
            .instance()
            .set(&VaultDataKey::BlendPool, &pool);
        Ok(())
    }

    /// Set the Blend factory contract address (admin only).
    pub fn set_blend_factory(
        env: Env,
        caller: SdkAddress,
        factory: SdkAddress,
    ) -> Result<(), ContractError> {
        require_admin(&env, &caller)?;
        env.storage()
            .instance()
            .set(&VaultDataKey::BlendFactory, &factory);
        Ok(())
    }

    /// Get the admin address.
    pub fn admin(env: Env) -> Result<SdkAddress, ContractError> {
        env.storage()
            .instance()
            .get(&VaultDataKey::Admin)
            .ok_or(ContractError::MissingConfig)
    }

    /// Get the asset token address.
    pub fn asset_token(env: Env) -> Result<SdkAddress, ContractError> {
        env.storage()
            .instance()
            .get(&VaultDataKey::AssetToken)
            .ok_or(ContractError::MissingConfig)
    }

    /// Get the share token address.
    pub fn share_token(env: Env) -> Result<SdkAddress, ContractError> {
        env.storage()
            .instance()
            .get(&VaultDataKey::ShareToken)
            .ok_or(ContractError::MissingConfig)
    }

    /// Get the Blend adapter contract address.
    pub fn blend_adapter(env: Env) -> Result<SdkAddress, ContractError> {
        env.storage()
            .instance()
            .get(&VaultDataKey::BlendAdapter)
            .ok_or(ContractError::MissingConfig)
    }

    /// Get the Blend pool contract address.
    pub fn blend_pool(env: Env) -> Result<SdkAddress, ContractError> {
        env.storage()
            .instance()
            .get(&VaultDataKey::BlendPool)
            .ok_or(ContractError::MissingConfig)
    }

    /// Get the Blend factory contract address.
    pub fn blend_factory(env: Env) -> Result<SdkAddress, ContractError> {
        env.storage()
            .instance()
            .get(&VaultDataKey::BlendFactory)
            .ok_or(ContractError::MissingConfig)
    }

    /// Check if the vault is paused.
    pub fn is_paused(env: Env) -> bool {
        let storage = SorobanStorage::new(&env);
        storage.is_paused()
    }

    /// Get total assets under management.
    pub fn total_assets(env: Env) -> i128 {
        use crate::storage::SorobanStorage;
        let storage = SorobanStorage::new(&env);
        storage
            .load_vault_state()
            .map(|s| s.total_assets)
            .unwrap_or(0)
    }

    /// Get total shares in circulation.
    pub fn total_shares(env: Env) -> i128 {
        use crate::storage::SorobanStorage;
        let storage = SorobanStorage::new(&env);
        storage
            .load_vault_state()
            .map(|s| s.total_shares)
            .unwrap_or(0)
    }

    /// Get idle assets (not deployed to markets).
    pub fn idle_assets(env: Env) -> i128 {
        use crate::storage::SorobanStorage;
        let storage = SorobanStorage::new(&env);
        storage
            .load_vault_state()
            .map(|s| s.idle_assets)
            .unwrap_or(0)
    }

    /// Get external assets (deployed to markets).
    pub fn external_assets(env: Env) -> i128 {
        use crate::storage::SorobanStorage;
        let storage = SorobanStorage::new(&env);
        storage
            .load_vault_state()
            .map(|s| s.external_assets)
            .unwrap_or(0)
    }

    /// Calculate shares for a given deposit amount.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment
    /// * `assets` - Amount of assets to deposit
    ///
    /// # Returns
    ///
    /// The number of shares that would be minted.
    pub fn preview_deposit(env: Env, assets: i128) -> Result<i128, ContractError> {
        if assets <= 0 {
            return Ok(0);
        }
        let assets_u128 =
            u128::try_from(assets).map_err(|_| ContractError::ConversionOverflow)?;

        let storage = SorobanStorage::new(&env);
        let state = storage
            .load_state()
            .map_err(ContractError::from)?
            .map(|versioned| versioned.state)
            .unwrap_or_default();
        let config = VaultConfig {
            fees: FeesSpec::zero(),
            min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
            max_pending_withdrawals: MAX_PENDING as u32,
            paused: storage.is_paused(),
            virtual_shares: 0,
            virtual_assets: 0,
        };
        let shares = preview_deposit_shares(&state, &config, assets_u128);
        let shares_i128 =
            i128::try_from(shares).map_err(|_| ContractError::ConversionOverflow)?;
        Ok(shares_i128)
    }

    /// Calculate assets for a given withdrawal amount.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment
    /// * `shares` - Amount of shares to redeem
    ///
    /// # Returns
    ///
    /// The number of assets that would be returned.
    pub fn preview_withdraw(env: Env, shares: i128) -> Result<i128, ContractError> {
        if shares <= 0 {
            return Ok(0);
        }
        let shares_u128 =
            u128::try_from(shares).map_err(|_| ContractError::ConversionOverflow)?;

        let storage = SorobanStorage::new(&env);
        let state = storage
            .load_state()
            .map_err(ContractError::from)?
            .map(|versioned| versioned.state)
            .unwrap_or_default();
        let config = VaultConfig {
            fees: FeesSpec::zero(),
            min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
            max_pending_withdrawals: MAX_PENDING as u32,
            paused: storage.is_paused(),
            virtual_shares: 0,
            virtual_assets: 0,
        };
        let assets = preview_withdraw_assets(&state, &config, shares_u128);
        let assets_i128 =
            i128::try_from(assets).map_err(|_| ContractError::ConversionOverflow)?;
        Ok(assets_i128)
    }

    /// Extend the TTL of contract storage.
    ///
    /// Call periodically to prevent state expiry.
    pub fn extend_ttl(env: Env) {
        extend_storage_ttl(&env);
    }
}

fn require_admin(env: &Env, caller: &SdkAddress) -> Result<(), ContractError> {
    caller.require_auth();
    let admin: SdkAddress = env
        .storage()
        .instance()
        .get(&VaultDataKey::Admin)
        .ok_or(ContractError::MissingConfig)?;
    if caller != &admin {
        return Err(ContractError::Unauthorized);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::PermissiveAuth;
    use crate::effects::MockInterpreter;
    use crate::storage::MemoryStorage;
    use alloc::vec;

    struct MockMarket;

    impl MarketAdapter for MockMarket {
        fn supply(&mut self, _market: MarketRef, _amount: u128) -> Result<(), RuntimeError> {
            Ok(())
        }

        fn withdraw(&mut self, _market: MarketRef, _amount: u128) -> Result<(), RuntimeError> {
            Ok(())
        }

        fn total_assets(&self, _market: MarketRef) -> Result<u128, RuntimeError> {
            Ok(1000)
        }
    }

    struct MockCrossChain;

    impl CrossChainMarketAdapter for MockCrossChain {
        fn submit_intent(
            &mut self,
            _plan_bytes: Vec<u8>,
        ) -> Result<crate::market::AttemptId, RuntimeError> {
            Ok(1)
        }

        fn settle(
            &mut self,
            op_id: u64,
            attempt_id: crate::market::AttemptId,
        ) -> Result<crate::market::SettlementReceipt, RuntimeError> {
            Ok(crate::market::SettlementReceipt {
                op_id,
                attempt_id,
                new_external_assets: 1000,
            })
        }

        fn total_assets(&self, _market: MarketRef) -> Result<u128, RuntimeError> {
            Ok(1000)
        }
    }

    fn test_config() -> ContractConfig {
        ContractConfig::new(
            [1u8; 32],
            [9u8; 32],
            vec![[2u8; 32]],
            vec![[3u8; 32]],
            [4u8; 32],
            [5u8; 32],
        )
    }

    fn create_test_vault(
    ) -> CuratorVault<MemoryStorage, PermissiveAuth, MockInterpreter, MockMarket, MockCrossChain>
    {
        let mut vault = CuratorVault::new(
            test_config(),
            MemoryStorage::new(),
            PermissiveAuth,
            MockInterpreter::new(),
            MockMarket,
            MockCrossChain,
        );
        vault.load_state().unwrap();
        vault
    }

    #[test]
    fn test_kernel_address_from_sdk_is_domain_separated() {
        use soroban_sdk::testutils::Address as _;

        let env = Env::default();
        let addr = SdkAddress::generate(&env);
        let derived = kernel_address_from_sdk(&env, &addr);

        let strkey = addr.to_string();
        let strkey_bytes = strkey.to_bytes();
        let mut strkey_vec = vec![0u8; strkey_bytes.len() as usize];
        strkey_bytes.copy_into_slice(&mut strkey_vec);
        let raw_bytes = Bytes::from_slice(&env, &strkey_vec);
        let raw_hash = env.crypto().sha256(&raw_bytes).to_bytes().to_array();

        let mut prefixed =
            Vec::with_capacity(KERNEL_ADDRESS_DOMAIN.len() + strkey_vec.len());
        prefixed.extend_from_slice(KERNEL_ADDRESS_DOMAIN);
        prefixed.extend_from_slice(&strkey_vec);
        let expected = env
            .crypto()
            .sha256(&Bytes::from_slice(&env, &prefixed))
            .to_bytes()
            .to_array();

        assert_eq!(derived, expected);
        assert_ne!(derived, raw_hash);
    }

    #[test]
    fn test_deposit_first() {
        let mut vault = create_test_vault();
        let caller = [1u8; 32];
        let receiver = [10u8; 32];

        let result = vault.deposit(caller, receiver, 1000, 0, 100).unwrap();

        assert_eq!(result.shares_minted, 1000);
        assert_eq!(result.total_shares, 1000);
        assert_eq!(result.total_assets, 1000);
    }

    #[test]
    fn test_deposit_subsequent() {
        let mut vault = create_test_vault();
        let caller = [1u8; 32];
        let receiver = [10u8; 32];

        // First deposit
        vault.deposit(caller, receiver, 1000, 0, 100).unwrap();

        // Second deposit should get proportional shares
        let result = vault.deposit(caller, receiver, 500, 0, 200).unwrap();

        assert_eq!(result.shares_minted, 500);
        assert_eq!(result.total_shares, 1500);
        assert_eq!(result.total_assets, 1500);
    }

    #[test]
    fn test_deposit_zero_fails() {
        let mut vault = create_test_vault();
        let caller = [1u8; 32];
        let receiver = [10u8; 32];

        let result = vault.deposit(caller, receiver, 0, 0, 100);

        assert!(result.is_err());
    }

    #[test]
    fn test_deposit_slippage_fails() {
        let mut vault = create_test_vault();
        let caller = [1u8; 32];
        let receiver = [10u8; 32];

        // Deposit with min_shares_out higher than actual
        let result = vault.deposit(caller, receiver, 1000, 2000, 100);

        assert!(result.is_err());
    }

    #[test]
    fn test_begin_allocating() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        let op_id = vault
            .begin_allocating(caller, vec![(0, 500), (1, 500)], 1000)
            .unwrap();

        assert_eq!(op_id, 0);
        assert!(vault.state().op_state.is_allocating());
    }

    #[test]
    fn test_finish_allocating() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        let op_id = vault.begin_allocating(caller, vec![(0, 500)], 1000).unwrap();

        let result = vault.finish_allocating(caller, op_id).unwrap();

        assert_eq!(result.op_id, op_id);
        assert!(vault.state().op_state.is_idle());
    }

    #[test]
    fn test_begin_refreshing() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        let op_id = vault.begin_refreshing(caller, vec![0, 1], 1000).unwrap();

        assert_eq!(op_id, 0);
        assert!(vault.state().op_state.is_refreshing());
    }

    #[test]
    fn test_finish_refreshing_reports_markets_refreshed() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        vault
            .acquire_market_lock(caller, 2, 5000, 1000)
            .expect("should acquire lock");

        let op_id = vault
            .begin_refreshing(caller, vec![0, 1, 2], 1500)
            .expect("should start refresh");

        let expected = vault
            .state()
            .op_state
            .as_refreshing()
            .expect("refreshing state")
            .plan
            .len() as u32;

        let result = vault.finish_refreshing(caller, op_id).unwrap();

        assert_eq!(result.markets_refreshed, expected);
        assert!(vault.state().op_state.is_idle());
    }

    #[test]
    fn test_sync_external_assets_in_allocating() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        let op_id = vault.begin_allocating(caller, vec![(0, 500)], 1000).unwrap();

        vault.sync_external_assets(caller, 1000, op_id, 1000).unwrap();

        assert_eq!(vault.state().external_assets, 1000);
    }

    #[test]
    fn test_execute_withdraw_respects_min_withdrawal_assets() {
        let mut vault = create_test_vault();
        let allocator = [3u8; 32];
        let owner = [1u8; 32];
        let receiver = [2u8; 32];

        let deposit_amount = MIN_WITHDRAWAL_ASSETS.saturating_mul(2);
        let request_time: u64 = 200;
        let exec_time = request_time
            .saturating_add(templar_vault_kernel::DEFAULT_COOLDOWN_NS)
            .saturating_add(1);

        vault
            .deposit(owner, receiver, deposit_amount, 0, request_time)
            .unwrap();

        vault
            .request_withdraw(owner, receiver, deposit_amount, 0, request_time)
            .unwrap();

        let (head_id, head_escrow_before, head_expected_before) = {
            let (id, head) = vault
                .state()
                .withdraw_queue
                .head()
                .expect("withdrawal queued");
            (id, head.escrow_shares, head.expected_assets)
        };

        {
            let state = vault.state_mut();
            state.idle_assets = MIN_WITHDRAWAL_ASSETS.saturating_sub(1);
            state.total_assets = state.idle_assets.saturating_add(state.external_assets);
        }

        let summary = vault.execute_withdraw(allocator, exec_time).unwrap();

        assert_eq!(summary.assets_transferred, 0);
        assert_eq!(summary.shares_burned, 0);
        assert!(vault.state().op_state.is_withdrawing());
        let (head_id_after, head_after) = vault
            .state()
            .withdraw_queue
            .head()
            .expect("withdrawal still queued");
        assert_eq!(head_id_after, head_id);
        assert_eq!(head_after.escrow_shares, head_escrow_before);
        assert_eq!(head_after.expected_assets, head_expected_before);
    }

    #[test]
    fn test_abort_allocating() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        // First deposit to have some idle assets
        vault.deposit([1u8; 32], [10u8; 32], 1000, 0, 100).unwrap();

        let op_id = vault.begin_allocating(caller, vec![(0, 500)], 1000).unwrap();

        vault.abort_allocating(caller, op_id, 500).unwrap();

        assert!(vault.state().op_state.is_idle());
    }

    #[test]
    fn test_contract_config() {
        let config = test_config();

        assert!(config.is_admin(&[1u8; 32]));
        assert!(!config.is_admin(&[2u8; 32]));

        assert!(config.is_guardian(&[2u8; 32]));
        assert!(!config.is_guardian(&[1u8; 32]));

        assert!(config.is_allocator(&[3u8; 32]));
        assert!(!config.is_allocator(&[1u8; 32]));

        assert!(config.is_privileged(&[1u8; 32])); // admin
        assert!(config.is_privileged(&[3u8; 32])); // allocator
        assert!(!config.is_privileged(&[2u8; 32])); // guardian only
    }

    #[test]
    fn test_reentrancy_guard_blocks_nested() {
        use soroban_sdk::testutils::Address as _;

        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(SorobanVaultContract, ());
        let admin = soroban_sdk::Address::generate(&env);
        let asset = soroban_sdk::Address::generate(&env);
        let share = soroban_sdk::Address::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(env.clone(), admin, asset, share).unwrap();
            let result = with_reentrancy_guard(&env, || {
                with_reentrancy_guard(&env, || Ok(()))
            });
            assert_eq!(result, Err(ContractError::Reentrancy));
        });
    }

    #[test]
    fn test_reentrancy_guard_resets_between_calls() {
        use soroban_sdk::testutils::Address as _;

        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(SorobanVaultContract, ());
        let admin = soroban_sdk::Address::generate(&env);
        let asset = soroban_sdk::Address::generate(&env);
        let share = soroban_sdk::Address::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(env.clone(), admin, asset, share).unwrap();
            with_reentrancy_guard(&env, || Ok(())).unwrap();
            with_reentrancy_guard(&env, || Ok(())).unwrap();
        });
    }

    #[test]
    fn test_loads_fees_spec_from_storage() {
        use soroban_sdk::testutils::Address as _;
        use templar_vault_kernel::fee::FeeSlot;
        use templar_vault_kernel::math::wad::Wad;

        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(SorobanVaultContract, ());
        let admin = soroban_sdk::Address::generate(&env);
        let asset = soroban_sdk::Address::generate(&env);
        let share = soroban_sdk::Address::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(env.clone(), admin, asset, share).unwrap();
        });

        let fees = FeesSpec::new(
            FeeSlot::new(Wad::one() / 10, [1u8; 32]),
            FeeSlot::new(Wad::one() / 20, [2u8; 32]),
            None,
        );

        env.as_contract(&contract_id, || {
            let bytes = borsh::to_vec(&fees).expect("fees serialize");
            env.storage()
                .instance()
                .set(&VaultDataKey::FeesSpec, &bytes);
        });

        env.as_contract(&contract_id, || {
            with_contract_vault(&env, |vault| {
                assert_eq!(vault.config.fees, fees);
                Ok(())
            })
            .unwrap();
        });
    }

    #[test]
    fn test_refresh_fees_mints_shares() {
        use templar_vault_kernel::fee::FeeSlot;
        use templar_vault_kernel::math::wad::Wad;

        let mut vault = create_test_vault();
        let fees = FeesSpec::new(
            FeeSlot::new(Wad::one() / 10, [9u8; 32]),
            FeeSlot::new(Wad::one() / 10, [8u8; 32]),
            None,
        );
        vault.config.fees = fees;

        {
            let state = vault.state_mut();
            state.total_assets = 1_500;
            state.total_shares = 1_000;
            state.idle_assets = 1_500;
            state.external_assets = 0;
            state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);
        }

        let annual_fee_assets = fees
            .management
            .fee_wad
            .apply_floored(Number::from(1_500u128));
        let mgmt_fee_assets = mul_div_floor(
            annual_fee_assets,
            Number::from(u128::from(YEAR_NS)),
            Number::from(u128::from(YEAR_NS)),
        );
        let mgmt_expected: u128 = compute_fee_shares_from_assets(
            mgmt_fee_assets,
            Number::from(1_500u128),
            Number::from(1_000u128),
        )
        .into();

        let total_supply_after_mgmt: u128 = 1_000u128 + mgmt_expected;
        let profit = 1_500u128.saturating_sub(1_000u128);
        let perf_fee_assets = fees
            .performance
            .fee_wad
            .apply_floored(Number::from(profit));
        let perf_expected: u128 = compute_fee_shares_from_assets(
            perf_fee_assets,
            Number::from(1_500u128),
            Number::from(total_supply_after_mgmt),
        )
        .into();

        let minted = vault.refresh_fees([1u8; 32], YEAR_NS).unwrap();

        assert_eq!(minted, mgmt_expected + perf_expected);
        assert_eq!(
            vault.state().total_shares,
            total_supply_after_mgmt + perf_expected
        );
        assert_eq!(vault.state().fee_anchor.total_assets, 1_500);
        assert_eq!(vault.state().fee_anchor.timestamp_ns, YEAR_NS);

        let mint_effects = vault
            .interpreter
            .effects
            .iter()
            .filter(|effect| matches!(effect, KernelEffect::MintShares { .. }))
            .count();
        assert_eq!(mint_effects, 2);
    }

    // =========================================================================
    // Policy tests
    // =========================================================================

    #[test]
    fn test_acquire_and_release_market_lock() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        // Acquire lock on market 1
        vault
            .acquire_market_lock(caller, 1, 5000, 1000)
            .expect("should acquire lock");

        // Market 1 should be locked
        assert!(vault.is_market_locked(1, 1500));
        // Market 2 should not be locked
        assert!(!vault.is_market_locked(2, 1500));

        // Release lock
        vault
            .release_market_lock(caller, 1)
            .expect("should release lock");

        // Market 1 should now be unlocked
        assert!(!vault.is_market_locked(1, 1500));
    }

    #[test]
    fn test_lock_expiry() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        // Acquire lock that expires at 2000
        vault
            .acquire_market_lock(caller, 1, 2000, 1000)
            .expect("should acquire lock");

        // Market 1 should be locked before expiry
        assert!(vault.is_market_locked(1, 1500));

        // Market 1 should be unlocked after expiry
        assert!(!vault.is_market_locked(1, 2500));
    }

    #[test]
    fn test_begin_allocating_filters_locked_markets() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        // Lock market 1
        vault
            .acquire_market_lock(caller, 1, 5000, 1000)
            .expect("should acquire lock");

        // Start allocation with markets 0, 1, 2 (1 is locked)
        let plan = vec![(0, 100), (1, 200), (2, 300)];
        let op_id = vault
            .begin_allocating(caller, plan, 1500)
            .expect("should start allocation");

        assert_eq!(op_id, 0);
        assert!(vault.state().op_state.is_allocating());

        // The allocation should have filtered out market 1
        // (We can't directly inspect the plan, but the operation should succeed)
    }

    #[test]
    fn test_begin_refreshing_filters_locked_markets() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        // Lock market 2
        vault
            .acquire_market_lock(caller, 2, 5000, 1000)
            .expect("should acquire lock");

        // Start refresh with markets 0, 1, 2 (2 is locked)
        let plan = vec![0, 1, 2];
        let op_id = vault
            .begin_refreshing(caller, plan, 1500)
            .expect("should start refresh");

        assert_eq!(op_id, 0);
        assert!(vault.state().op_state.is_refreshing());
    }

    #[test]
    fn test_allocating_all_locked_markets() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        // Lock both markets in the plan
        vault.acquire_market_lock(caller, 0, 5000, 1000).unwrap();
        vault.acquire_market_lock(caller, 1, 5000, 1000).unwrap();

        // Start allocation with only locked markets - results in empty plan
        // The kernel rejects empty allocation plans
        let plan = vec![(0, 100), (1, 200)];
        let result = vault.begin_allocating(caller, plan, 1500);

        // Empty plan is rejected by kernel
        assert!(result.is_err());
        // Vault should still be in idle state
        assert!(vault.state().op_state.is_idle());
    }

    #[test]
    fn test_policy_state_getter() {
        let vault = create_test_vault();

        // Policy state should be initialized empty
        assert!(vault.policy_state().locks.is_empty());
        assert!(vault.policy_state().markets.is_empty());
        assert!(vault.policy_state().principals.is_empty());
        assert!(vault.policy_state().cap_groups.is_empty());
    }

    #[test]
    fn test_load_state_restores_policy_and_restrictions() {
        use crate::policy::MarketLock;
        use std::collections::BTreeSet;
        use soroban_sdk::testutils::Address as _;

        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(SorobanVaultContract, ());
        let admin = soroban_sdk::Address::generate(&env);
        let asset = soroban_sdk::Address::generate(&env);
        let share = soroban_sdk::Address::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(env.clone(), admin, asset, share).unwrap();

            let mut storage = SorobanStorage::new(&env);
            let versioned = VersionedState::new(VaultState::default());
            storage.save_state(&versioned).unwrap();
            storage.save_paused(false).unwrap();

            let mut policy_state = PolicyState::new();
            let lock = MarketLock::new(1, 10).with_expiry(20);
            policy_state.locks = policy_state.locks.acquire(lock, 10).unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();

            let mut blacklist = BTreeSet::new();
            blacklist.insert([9u8; 32]);
            let restrictions = Restrictions::BlackList(blacklist);
            Storage::save_restrictions(&mut storage, &Some(restrictions.clone())).unwrap();

            let mut vault = CuratorVault::new(
                test_config(),
                storage,
                PermissiveAuth,
                MockInterpreter::new(),
                MockMarket,
                MockCrossChain,
            );
            vault.load_state().unwrap();

            assert!(vault.is_market_locked(1, 10));
            assert_eq!(vault.restrictions(), Some(&restrictions));
        });
    }
}
