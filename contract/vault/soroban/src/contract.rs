//! Soroban curator vault contract entrypoints.
//!
//! This module provides the contract entrypoints that map to kernel actions.
//! Each entrypoint performs authorization, dispatches to kernel transitions,
//! and executes the returned effects.

use alloc::string::String;
use alloc::vec::Vec;
use templar_curator_primitives::{determine_recovery_action, RecoveryContext};
use templar_vault_kernel::{
    apply_action, complete_allocation, complete_refresh, start_allocation, start_refresh, Address,
    Fee, Fees, KernelAction, OpState, PayoutOutcome, TargetId, VaultConfig, VaultState, Wad,
    MAX_PENDING, MIN_WITHDRAWAL_ASSETS,
};

use crate::auth::{ActionKind, AuthAdapter};
use crate::effects::{EffectContext, EffectInterpreter, EffectSummary};
use crate::error::RuntimeError;
use crate::market::{CrossChainMarketAdapter, MarketAdapter, MarketRef};
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
        }
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
            fees: Fees {
                performance: Fee {
                    fee: Wad::ZERO,
                    recipient: String::new(),
                },
                management: Fee {
                    fee: Wad::ZERO,
                    recipient: String::new(),
                },
                max_total_assets_growth_rate: None,
            },
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
        let state = self.state().clone();
        let result = apply_action(state, &config, None, &self.config.admin, action)
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

        if assets == 0 {
            return Err(RuntimeError::contract_error("deposit amount is zero"));
        }

        let state = self.state_mut();

        // Calculate shares using simple 1:1 ratio for initial deposits
        // or proportional for subsequent deposits
        let shares = if state.total_shares == 0 {
            assets // 1:1 for first deposit
        } else {
            // shares = assets * total_shares / total_assets
            assets
                .checked_mul(state.total_shares)
                .and_then(|n| n.checked_div(state.total_assets))
                .ok_or_else(|| RuntimeError::contract_error("overflow in share calculation"))?
        };

        if shares < min_shares_out {
            return Err(RuntimeError::contract_error("slippage exceeded"));
        }

        // Update state
        state.total_assets = state.total_assets.saturating_add(assets);
        state.total_shares = state.total_shares.saturating_add(shares);
        state.idle_assets = state.idle_assets.saturating_add(assets);

        // Create and execute effects
        let ctx = self.effect_context(now_ns);
        let effect = templar_vault_kernel::effects::KernelEffect::MintShares {
            owner: receiver,
            shares,
        };
        self.interpreter.execute_effect(&effect, &ctx)?;

        self.save_state()?;

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
        _receiver: Address,
        shares: u128,
        min_assets_out: u128,
        now_ns: u64,
    ) -> Result<WithdrawRequestResult, RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::RequestWithdraw, caller, None)?;

        if self.auth.is_paused() {
            return Err(RuntimeError::contract_error("vault is paused"));
        }

        if shares == 0 {
            return Err(RuntimeError::contract_error("withdrawal shares is zero"));
        }

        let state = self.state_mut();

        // Calculate assets for shares
        let assets = if state.total_shares == 0 {
            return Err(RuntimeError::contract_error("no shares in vault"));
        } else {
            shares
                .checked_mul(state.total_assets)
                .and_then(|n| n.checked_div(state.total_shares))
                .ok_or_else(|| RuntimeError::contract_error("overflow in asset calculation"))?
        };

        if assets < min_assets_out {
            return Err(RuntimeError::contract_error("slippage exceeded"));
        }

        // Generate request ID
        let request_id = state.next_op_id;
        state.next_op_id = state.next_op_id.saturating_add(1);

        // Escrow shares (transfer from owner to escrow via effect)
        let ctx = self.effect_context(now_ns);
        let escrow_address = [0u8; 32]; // Placeholder escrow address
        let effect = templar_vault_kernel::effects::KernelEffect::TransferShares {
            from: caller,
            to: escrow_address,
            shares,
        };
        self.interpreter.execute_effect(&effect, &ctx)?;

        self.save_state()?;

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
        _now_ns: u64,
    ) -> Result<EffectSummary, RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::ExecuteWithdraw, caller, None)?;

        // Get current state
        let state = self.state();

        // Check if we're in a state that allows withdrawal execution
        if !state.op_state.is_idle() {
            return Err(RuntimeError::contract_error(
                "vault not in idle state for withdrawal",
            ));
        }

        // For now, return empty summary - actual implementation would
        // process the withdrawal queue
        Ok(EffectSummary::new())
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

    // =========================================================================
    // Privileged entrypoints (internal/runtime)
    // =========================================================================

    /// Begin an allocation operation.
    ///
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `plan` - Allocation plan: list of (target_id, amount) pairs
    pub fn begin_allocating(
        &mut self,
        caller: Address,
        plan: Vec<(TargetId, u128)>,
    ) -> Result<u64, RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::BeginAllocating, caller, None)?;

        let state = self.state_mut();
        let op_id = state.next_op_id;
        state.next_op_id = state.next_op_id.saturating_add(1);

        // Call kernel transition
        let result = start_allocation(state.op_state.clone(), plan, op_id)
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
    pub fn sync_external_assets(
        &mut self,
        caller: Address,
        new_external_assets: u128,
        op_id: u64,
    ) -> Result<(), RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::SyncExternalAssets, caller, None)?;

        let state = self.state_mut();

        // Verify we're in an operation that expects sync
        let current_op_id = match &state.op_state {
            OpState::Allocating(s) => s.op_id,
            OpState::Refreshing(s) => s.op_id,
            _ => {
                return Err(RuntimeError::contract_error(
                    "not in allocating or refreshing state",
                ))
            }
        };

        if current_op_id != op_id {
            return Err(RuntimeError::contract_error("op_id mismatch"));
        }

        // Update external assets
        let old_external = state.external_assets;
        state.external_assets = new_external_assets;

        // Adjust total_assets based on the change
        if new_external_assets > old_external {
            let increase = new_external_assets - old_external;
            state.total_assets = state.total_assets.saturating_add(increase);
        } else {
            let decrease = old_external - new_external_assets;
            state.total_assets = state.total_assets.saturating_sub(decrease);
        }

        self.save_state()?;
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
    /// # Arguments
    ///
    /// * `caller` - The caller's address (must be allocator)
    /// * `plan` - List of target IDs to refresh
    pub fn begin_refreshing(
        &mut self,
        caller: Address,
        plan: Vec<TargetId>,
    ) -> Result<u64, RuntimeError> {
        // Authorize
        self.auth
            .authorize(ActionKind::BeginRefreshing, caller, None)?;

        let state = self.state_mut();
        let op_id = state.next_op_id;
        state.next_op_id = state.next_op_id.saturating_add(1);

        // Call kernel transition
        let result = start_refresh(state.op_state.clone(), plan, op_id)
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
        now_ns: u64,
    ) -> Result<Option<EffectSummary>, RuntimeError> {
        let Some(action) = determine_recovery_action(&self.state().op_state, &context) else {
            return Ok(None);
        };

        let kind: ActionKind = (&action).into();
        self.auth.authorize(kind, caller, None)?;

        let summary = self.apply_kernel_action(action, now_ns)?;
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

            // Build plan from markets
            let plan: Vec<TargetId> = markets.iter().map(|m| m.market_id).collect();

            // Start refresh
            let result = start_refresh(state.op_state.clone(), plan, op_id)
                .map_err(RuntimeError::transition_error)?;
            state.op_state = result.new_state;

            op_id
        };

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
            .begin_allocating(caller, vec![(0, 500), (1, 500)])
            .unwrap();

        assert_eq!(op_id, 0);
        assert!(vault.state().op_state.is_allocating());
    }

    #[test]
    fn test_finish_allocating() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        let op_id = vault.begin_allocating(caller, vec![(0, 500)]).unwrap();

        let result = vault.finish_allocating(caller, op_id).unwrap();

        assert_eq!(result.op_id, op_id);
        assert!(vault.state().op_state.is_idle());
    }

    #[test]
    fn test_begin_refreshing() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        let op_id = vault.begin_refreshing(caller, vec![0, 1]).unwrap();

        assert_eq!(op_id, 0);
        assert!(vault.state().op_state.is_refreshing());
    }

    #[test]
    fn test_sync_external_assets_in_allocating() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        let op_id = vault.begin_allocating(caller, vec![(0, 500)]).unwrap();

        vault.sync_external_assets(caller, 1000, op_id).unwrap();

        assert_eq!(vault.state().external_assets, 1000);
    }

    #[test]
    fn test_abort_allocating() {
        let mut vault = create_test_vault();
        let caller = [3u8; 32]; // allocator

        // First deposit to have some idle assets
        vault.deposit([1u8; 32], [10u8; 32], 1000, 0, 100).unwrap();

        let op_id = vault.begin_allocating(caller, vec![(0, 500)]).unwrap();

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
}
