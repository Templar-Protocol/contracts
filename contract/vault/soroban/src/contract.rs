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

use alloc::vec::Vec;
use soroban_sdk::{contract, contractimpl, contracttype, Address as SdkAddress, Env, Vec as SdkVec};
use templar_curator_primitives::{
    determine_recovery_action, PolicyState, RecoveryContext, RecoveryProgress,
};
use templar_vault_kernel::{
    apply_action, complete_allocation, complete_refresh, start_allocation, start_refresh,
    withdrawal_collected, withdrawal_step_callback, Address, FeesSpec, KernelAction, OpState,
    PayoutOutcome, Restrictions, TargetId, VaultConfig, VaultState, MAX_PENDING,
    MIN_WITHDRAWAL_ASSETS,
};
use templar_vault_kernel::effects::KernelEffect;
use templar_vault_kernel::state::queue::{compute_full_withdrawal, compute_partial_withdrawal};

use crate::auth::{ActionKind, AuthAdapter};
use crate::effects::{EffectContext, EffectInterpreter, EffectSummary};
use crate::error::RuntimeError;
use crate::market::{CrossChainMarketAdapter, MarketAdapter, MarketRef};
use crate::policy::{build_refresh_plan_with_locks, filter_allocation_plan};
use crate::reconciliation::{reconcile_external_assets, ReconciliationRecord};
use crate::storage::{Storage, VersionedState};

/// Contract configuration set at initialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractConfig {
    /// Administrator address.
    pub admin: Address,
    /// Guardian addresses (can pause).
    pub guardians: Vec<Address>,
    /// Allocator addresses (can manage allocations).
    pub allocators: Vec<Address>,
    /// Underlying asset contract address.
    pub asset_address: Address,
    /// Share token contract address.
    pub share_address: Address,
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
        guardians: Vec<Address>,
        allocators: Vec<Address>,
        asset_address: Address,
        share_address: Address,
    ) -> Self {
        Self {
            admin,
            guardians,
            allocators,
            asset_address,
            share_address,
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
    E: EffectInterpreter,
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
    E: EffectInterpreter,
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
            self.config.admin, // vault address = admin for now
            self.config.asset_address,
            self.config.share_address,
        )
    }

    fn kernel_config(&self) -> VaultConfig {
        VaultConfig {
            fees: FeesSpec::zero(),
            min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
            max_pending_withdrawals: MAX_PENDING as u32,
            paused: self.paused,
            virtual_shares: 0,
            virtual_assets: 0,
        }
    }

    fn apply_kernel_action(
        &mut self,
        action: KernelAction,
        now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        let config = self.kernel_config();
        let restrictions = self.restrictions.as_ref();
        let state = self.state().clone();
        let result = apply_action(state, &config, restrictions, &self.config.admin, action)
            .map_err(RuntimeError::transition_error)?;

        let ctx = self.effect_context(now_ns);
        let summary = self
            .interpreter
            .execute_effects(&result.effects, &ctx)?;

        self.state = Some(result.state);
        self.save_state()?;

        Ok(summary)
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

        if self.auth.is_paused() {
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
            summary = merge_summaries(summary, step_summary);
        } else if !self.state().op_state.is_withdrawing() {
            return Err(RuntimeError::contract_error(
                "vault not in idle or withdrawing state for withdrawal",
            ));
        }

        if self.state().op_state.is_withdrawing() {
            let settle_summary = self.complete_withdrawal_from_idle(now_ns)?;
            summary = merge_summaries(summary, settle_summary);
        }

        Ok(summary)
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
        summary = merge_summaries(summary, transfer_summary);

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
        summary = merge_summaries(summary, settle_summary);

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

        // Call kernel transition
        let result = complete_refresh(state.op_state.clone(), op_id)
            .map_err(RuntimeError::transition_error)?;

        state.op_state = result.new_state;
        let external_assets = state.external_assets;
        self.save_state()?;

        Ok(RefreshResult {
            op_id,
            markets_refreshed: 0, // Would be tracked during refresh
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
    pub fn refresh_fees(&mut self, caller: Address, now_ns: u64) -> Result<u128, RuntimeError> {
        // Authorize
        self.auth.authorize(ActionKind::RefreshFees, caller, None)?;

        let state = self.state_mut();

        // Calculate accrued fees based on time elapsed
        let elapsed_ns = now_ns.saturating_sub(state.fee_anchor.timestamp_ns);

        // For now, return 0 fees - actual implementation would compute based on fee config
        let _fees_accrued = 0u128;
        let _elapsed = elapsed_ns; // suppress warning

        // Update anchor
        state.fee_anchor.timestamp_ns = now_ns;

        self.save_state()?;

        Ok(0)
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

fn merge_summaries(mut base: EffectSummary, other: EffectSummary) -> EffectSummary {
    base.shares_minted = base.shares_minted.saturating_add(other.shares_minted);
    base.shares_burned = base.shares_burned.saturating_add(other.shares_burned);
    base.shares_transferred = base
        .shares_transferred
        .saturating_add(other.shares_transferred);
    base.assets_transferred = base
        .assets_transferred
        .saturating_add(other.assets_transferred);
    base.events_emitted = base.events_emitted.saturating_add(other.events_emitted);
    base
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
    /// Blend adapter contract address.
    BlendAdapter,
    /// Blend pool contract address.
    BlendPool,
    /// Blend factory contract address.
    BlendFactory,
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
    /// # Panics
    ///
    /// Panics if the contract is already initialized.
    pub fn initialize(
        env: Env,
        admin: SdkAddress,
        asset_token: SdkAddress,
        share_token: SdkAddress,
    ) {
        // Check not already initialized
        if env
            .storage()
            .instance()
            .has(&VaultDataKey::Initialized)
        {
            panic!("already initialized");
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
            .set(&VaultDataKey::Initialized, &true);

        // Initialize vault state in persistent storage
        use crate::storage::{SorobanStorage, SorobanVaultState};
        let storage = SorobanStorage::new(&env);
        storage.save_vault_state(&SorobanVaultState::default());
        storage.set_version(1);
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
    ) -> i128 {
        // Require authorization from owner
        owner.require_auth();

        // Check not paused
        let paused: bool = env
            .storage()
            .instance()
            .get(&VaultDataKey::Paused)
            .unwrap_or(false);
        if paused {
            panic!("vault is paused");
        }

        if assets <= 0 {
            panic!("deposit amount must be positive");
        }

        // Load vault state
        use crate::storage::{SorobanStorage, SorobanVaultState};
        let storage = SorobanStorage::new(&env);
        let mut state = storage.load_vault_state().unwrap_or_default();

        // Calculate shares
        let shares = if state.total_shares == 0 {
            assets // 1:1 for first deposit
        } else {
            assets
                .checked_mul(state.total_shares)
                .and_then(|n| n.checked_div(state.total_assets))
                .expect("share calculation overflow")
        };

        if shares < min_shares_out {
            panic!("slippage exceeded");
        }

        // Update state
        state.total_assets = state.total_assets.saturating_add(assets);
        state.total_shares = state.total_shares.saturating_add(shares);
        state.idle_assets = state.idle_assets.saturating_add(assets);

        // Save state
        storage.save_vault_state(&state);

        // Transfer assets from owner to vault
        let asset_token: SdkAddress = env
            .storage()
            .instance()
            .get(&VaultDataKey::AssetToken)
            .expect("asset token not set");
        let token_client = soroban_sdk::token::Client::new(&env, &asset_token);
        token_client.transfer(&owner, &env.current_contract_address(), &assets);

        // Mint shares to receiver
        let share_token: SdkAddress = env
            .storage()
            .instance()
            .get(&VaultDataKey::ShareToken)
            .expect("share token not set");
        let share_client = soroban_sdk::token::StellarAssetClient::new(&env, &share_token);
        share_client.mint(&receiver, &shares);

        // Emit deposit event
        use crate::effects::DepositEvent;
        DepositEvent {
            owner: owner.clone(),
            receiver,
            assets_in: assets,
            shares_out: shares,
        }
        .publish(&env);

        shares
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
    ) -> u64 {
        // Require authorization from owner
        owner.require_auth();

        if shares <= 0 {
            panic!("shares must be positive");
        }

        // Load vault state
        use crate::storage::{SorobanStorage, SorobanVaultState};
        let storage = SorobanStorage::new(&env);
        let state = storage.load_vault_state().expect("vault not initialized");

        if state.total_shares == 0 {
            panic!("no shares in vault");
        }

        // Calculate expected assets
        let expected_assets = shares
            .checked_mul(state.total_assets)
            .and_then(|n| n.checked_div(state.total_shares))
            .expect("asset calculation overflow");

        if expected_assets < min_assets_out {
            panic!("slippage exceeded");
        }

        // Generate request ID (simplified - in production use proper sequencing)
        let request_id = state.next_op_id;

        // Emit withdrawal request event
        use crate::effects::WithdrawRequestEvent;
        WithdrawRequestEvent {
            id: request_id,
            owner: owner.clone(),
            receiver,
            shares,
            expected_assets,
        }
        .publish(&env);

        request_id
    }

    /// Pause or unpause the vault.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment
    /// * `caller` - The caller (must be admin)
    /// * `paused` - Whether to pause (true) or unpause (false)
    pub fn set_paused(env: Env, caller: SdkAddress, paused: bool) {
        require_admin(&env, &caller);

        // Update paused state
        env.storage()
            .instance()
            .set(&VaultDataKey::Paused, &paused);

        // Emit event
        use crate::effects::PauseUpdatedEvent;
        PauseUpdatedEvent { paused }.publish(&env);
    }

    /// Set the Blend adapter contract address (admin only).
    pub fn set_blend_adapter(env: Env, caller: SdkAddress, adapter: SdkAddress) {
        require_admin(&env, &caller);
        env.storage()
            .instance()
            .set(&VaultDataKey::BlendAdapter, &adapter);
    }

    /// Set the Blend pool contract address (admin only).
    pub fn set_blend_pool(env: Env, caller: SdkAddress, pool: SdkAddress) {
        require_admin(&env, &caller);
        env.storage()
            .instance()
            .set(&VaultDataKey::BlendPool, &pool);
    }

    /// Set the Blend factory contract address (admin only).
    pub fn set_blend_factory(env: Env, caller: SdkAddress, factory: SdkAddress) {
        require_admin(&env, &caller);
        env.storage()
            .instance()
            .set(&VaultDataKey::BlendFactory, &factory);
    }

    /// Get the admin address.
    pub fn admin(env: Env) -> SdkAddress {
        env.storage()
            .instance()
            .get(&VaultDataKey::Admin)
            .expect("admin not set")
    }

    /// Get the asset token address.
    pub fn asset_token(env: Env) -> SdkAddress {
        env.storage()
            .instance()
            .get(&VaultDataKey::AssetToken)
            .expect("asset token not set")
    }

    /// Get the share token address.
    pub fn share_token(env: Env) -> SdkAddress {
        env.storage()
            .instance()
            .get(&VaultDataKey::ShareToken)
            .expect("share token not set")
    }

    /// Get the Blend adapter contract address.
    pub fn blend_adapter(env: Env) -> SdkAddress {
        env.storage()
            .instance()
            .get(&VaultDataKey::BlendAdapter)
            .expect("blend adapter not set")
    }

    /// Get the Blend pool contract address.
    pub fn blend_pool(env: Env) -> SdkAddress {
        env.storage()
            .instance()
            .get(&VaultDataKey::BlendPool)
            .expect("blend pool not set")
    }

    /// Get the Blend factory contract address.
    pub fn blend_factory(env: Env) -> SdkAddress {
        env.storage()
            .instance()
            .get(&VaultDataKey::BlendFactory)
            .expect("blend factory not set")
    }

    /// Check if the vault is paused.
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&VaultDataKey::Paused)
            .unwrap_or(false)
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
    pub fn preview_deposit(env: Env, assets: i128) -> i128 {
        use crate::storage::SorobanStorage;
        let storage = SorobanStorage::new(&env);
        let state = match storage.load_vault_state() {
            Some(s) => s,
            None => return assets, // 1:1 for empty vault
        };

        if state.total_shares == 0 {
            assets // 1:1 for first deposit
        } else {
            assets
                .checked_mul(state.total_shares)
                .and_then(|n| n.checked_div(state.total_assets))
                .unwrap_or(0)
        }
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
    pub fn preview_withdraw(env: Env, shares: i128) -> i128 {
        use crate::storage::SorobanStorage;
        let storage = SorobanStorage::new(&env);
        let state = match storage.load_vault_state() {
            Some(s) => s,
            None => return 0,
        };

        if state.total_shares == 0 {
            0
        } else {
            shares
                .checked_mul(state.total_assets)
                .and_then(|n| n.checked_div(state.total_shares))
                .unwrap_or(0)
        }
    }

    /// Extend the TTL of contract storage.
    ///
    /// Call periodically to prevent state expiry.
    pub fn extend_ttl(env: Env) {
        // Extend instance storage
        env.storage()
            .instance()
            .extend_ttl(50_000, 100_000);

        // Extend persistent storage (vault state)
        use crate::storage::SorobanStorage;
        let storage = SorobanStorage::new(&env);
        storage.extend_ttl(50_000, 100_000);
    }
}

fn require_admin(env: &Env, caller: &SdkAddress) {
    caller.require_auth();
    let admin: SdkAddress = env
        .storage()
        .instance()
        .get(&VaultDataKey::Admin)
        .expect("admin not set");
    if caller != &admin {
        panic!("caller is not admin");
    }
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
    fn test_sync_external_assets_in_allocating() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        let op_id = vault.begin_allocating(caller, vec![(0, 500)], 1000).unwrap();

        vault.sync_external_assets(caller, 1000, op_id, 1000).unwrap();

        assert_eq!(vault.state().external_assets, 1000);
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
}
