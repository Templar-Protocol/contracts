//! Effect interpreter for processing kernel effects on Soroban.
//!
//! This module provides the [`EffectInterpreter`] trait and supporting types
//! for executing kernel effects on the Soroban blockchain.

use alloc::vec::Vec;
use soroban_sdk::{contractevent, token::StellarAssetClient, Address, Env};
use templar_vault_kernel::effects::KernelEffect;

use crate::error::RuntimeError;

// ---------------------------------------------------------------------------
// Contract Events (using #[contractevent] structs)
// ---------------------------------------------------------------------------

/// Deposit processed event.
#[contractevent]
pub struct DepositEvent {
    /// Owner who deposited.
    #[topic]
    pub owner: Address,
    /// Receiver of shares.
    #[topic]
    pub receiver: Address,
    /// Assets deposited.
    pub assets_in: i128,
    /// Shares minted.
    pub shares_out: i128,
}

/// Withdrawal requested event.
#[contractevent]
pub struct WithdrawRequestEvent {
    /// Request ID.
    #[topic]
    pub id: u64,
    /// Owner requesting withdrawal.
    #[topic]
    pub owner: Address,
    /// Receiver of assets.
    pub receiver: Address,
    /// Shares to burn.
    pub shares: i128,
    /// Expected assets out.
    pub expected_assets: i128,
}

/// Withdrawal started event.
#[contractevent]
pub struct WithdrawStartEvent {
    /// Operation ID.
    #[topic]
    pub op_id: u64,
    /// Amount being withdrawn.
    pub amount: i128,
    /// Shares in escrow.
    pub escrow_shares: i128,
    /// Owner.
    pub owner: Address,
    /// Receiver.
    pub receiver: Address,
}

/// Withdrawal collected event.
#[contractevent]
pub struct WithdrawCollectedEvent {
    /// Operation ID.
    #[topic]
    pub op_id: u64,
    /// Shares burned.
    pub burn_shares: i128,
    /// Amount collected.
    pub collected: i128,
}

/// Withdrawal stopped event.
#[contractevent]
pub struct WithdrawStoppedEvent {
    /// Operation ID.
    #[topic]
    pub op_id: u64,
    /// Escrowed shares returned.
    pub escrow_shares: i128,
}

/// Payout completed event.
#[contractevent]
pub struct PayoutEvent {
    /// Operation ID.
    #[topic]
    pub op_id: u64,
    /// Whether payout succeeded.
    pub success: bool,
    /// Shares burned.
    pub burn_shares: i128,
    /// Shares refunded.
    pub refund_shares: i128,
    /// Amount paid out.
    pub amount: i128,
}

/// Allocation started event.
#[contractevent]
pub struct AllocStartEvent {
    /// Operation ID.
    #[topic]
    pub op_id: u64,
    /// Total to allocate.
    pub total: i128,
    /// Number of plan steps.
    pub plan_len: u32,
}

/// Allocation step failed event.
#[contractevent]
pub struct AllocStepFailEvent {
    /// Operation ID.
    #[topic]
    pub op_id: u64,
    /// Step index.
    pub index: u32,
    /// Remaining amount.
    pub remaining: i128,
}

/// Allocation completed event.
#[contractevent]
pub struct AllocDoneEvent {
    /// Operation ID.
    #[topic]
    pub op_id: u64,
    /// Whether triggered by withdrawal.
    pub has_withdrawal: bool,
}

/// Refresh started event.
#[contractevent]
pub struct RefreshStartEvent {
    /// Operation ID.
    #[topic]
    pub op_id: u64,
    /// Plan length.
    pub plan_len: u32,
}

/// Refresh completed event.
#[contractevent]
pub struct RefreshDoneEvent {
    /// Operation ID.
    #[topic]
    pub op_id: u64,
}

/// External assets synced event.
#[contractevent]
pub struct ExtAssetsSyncEvent {
    /// Operation ID.
    #[topic]
    pub op_id: u64,
    /// New external assets value.
    pub new_external_assets: i128,
    /// Total assets.
    pub total_assets: i128,
}

/// Fees refreshed event.
#[contractevent]
pub struct FeesRefreshEvent {
    /// Timestamp.
    pub now_ns: u64,
    /// Total assets.
    pub total_assets: i128,
}

/// Pause state updated event.
#[contractevent]
pub struct PauseUpdatedEvent {
    /// New pause state.
    pub paused: bool,
}

/// Result type for effect operations.
pub type EffectResult<T> = Result<T, RuntimeError>;

// ---------------------------------------------------------------------------
// Effect Context
// ---------------------------------------------------------------------------

/// Context provided to effect handlers.
///
/// Contains information about the current execution environment
/// that effect handlers may need.
#[derive(Clone, Debug)]
pub struct EffectContext {
    /// Current timestamp in nanoseconds.
    pub now_ns: u64,
    /// The vault contract address (kernel format).
    pub vault_address: templar_vault_kernel::Address,
    /// The underlying asset contract address (kernel format).
    pub asset_address: templar_vault_kernel::Address,
    /// The share token contract address (kernel format).
    pub share_address: templar_vault_kernel::Address,
}

impl EffectContext {
    /// Create a new effect context.
    #[inline]
    #[must_use]
    pub fn new(
        now_ns: u64,
        vault_address: templar_vault_kernel::Address,
        asset_address: templar_vault_kernel::Address,
        share_address: templar_vault_kernel::Address,
    ) -> Self {
        Self {
            now_ns,
            vault_address,
            asset_address,
            share_address,
        }
    }
}

// ---------------------------------------------------------------------------
// Effect Summary
// ---------------------------------------------------------------------------

/// Effect execution summary.
///
/// Tracks the cumulative results of executing a batch of effects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EffectSummary {
    /// Total shares minted.
    pub shares_minted: u128,
    /// Total shares burned.
    pub shares_burned: u128,
    /// Total shares transferred.
    pub shares_transferred: u128,
    /// Total assets transferred out.
    pub assets_transferred: u128,
    /// Number of events emitted.
    pub events_emitted: u32,
}

impl EffectSummary {
    /// Create a new empty summary.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            shares_minted: 0,
            shares_burned: 0,
            shares_transferred: 0,
            assets_transferred: 0,
            events_emitted: 0,
        }
    }

    /// Record a mint effect.
    #[inline]
    pub fn record_mint(&mut self, shares: u128) {
        self.shares_minted = self.shares_minted.saturating_add(shares);
    }

    /// Record a burn effect.
    #[inline]
    pub fn record_burn(&mut self, shares: u128) {
        self.shares_burned = self.shares_burned.saturating_add(shares);
    }

    /// Record a share transfer effect.
    #[inline]
    pub fn record_share_transfer(&mut self, shares: u128) {
        self.shares_transferred = self.shares_transferred.saturating_add(shares);
    }

    /// Record an asset transfer effect.
    #[inline]
    pub fn record_asset_transfer(&mut self, amount: u128) {
        self.assets_transferred = self.assets_transferred.saturating_add(amount);
    }

    /// Record an event emission.
    #[inline]
    pub fn record_event(&mut self) {
        self.events_emitted = self.events_emitted.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Effect Interpreter Trait
// ---------------------------------------------------------------------------

/// Trait for interpreting and executing kernel effects.
///
/// Implementations of this trait execute effects on the actual blockchain
/// (Soroban ledger, token contracts, etc.).
///
/// # Effect Types
///
/// - `MintShares` - Create new share tokens for an owner.
/// - `BurnShares` - Destroy share tokens from an owner.
/// - `TransferShares` - Move share tokens between accounts.
/// - `TransferAssets` - Transfer underlying assets to a recipient.
/// - `EmitEvent` - Emit an event to the blockchain.
///
/// Note: `ExternalCall` and `ChargeStorage` are feature-gated for NEAR only
/// and are not present in Soroban builds.
pub trait EffectInterpreter {
    /// Execute a single kernel effect.
    ///
    /// # Arguments
    ///
    /// * `effect` - The effect to execute.
    /// * `ctx` - The execution context.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, `Err(RuntimeError)` on failure.
    fn execute_effect(&mut self, effect: &KernelEffect, ctx: &EffectContext) -> EffectResult<()>;

    /// Execute a batch of kernel effects in order.
    ///
    /// Effects are executed sequentially in the order provided.
    /// If any effect fails, execution stops and the error is returned.
    ///
    /// # Arguments
    ///
    /// * `effects` - The effects to execute.
    /// * `ctx` - The execution context.
    ///
    /// # Returns
    ///
    /// A summary of all executed effects on success, or an error if any effect failed.
    fn execute_effects(
        &mut self,
        effects: &[KernelEffect],
        ctx: &EffectContext,
    ) -> EffectResult<EffectSummary> {
        let mut summary = EffectSummary::new();

        for effect in effects {
            self.execute_effect(effect, ctx)?;

            // Update summary based on effect type
            match effect {
                KernelEffect::MintShares { shares, .. } => {
                    summary.record_mint(*shares);
                }
                KernelEffect::BurnShares { shares, .. } => {
                    summary.record_burn(*shares);
                }
                KernelEffect::TransferShares { shares, .. } => {
                    summary.record_share_transfer(*shares);
                }
                KernelEffect::TransferAssets { amount, .. } => {
                    summary.record_asset_transfer(*amount);
                }
                KernelEffect::EmitEvent { .. } => {
                    summary.record_event();
                }
                #[allow(unreachable_patterns)]
                _ => {}
            }
        }

        Ok(summary)
    }
}

// ---------------------------------------------------------------------------
// SEP-41 Token Interface
// ---------------------------------------------------------------------------

/// SEP-41 Token trait for Soroban token operations.
///
/// This trait abstracts over SEP-41 compliant token contracts (Stellar Asset Contract).
/// Implementations handle the actual blockchain calls for minting, burning, and transferring.
///
/// SEP-41 uses i128 for amounts.
pub trait Sep41Token {
    /// Mint tokens to an address.
    ///
    /// # Arguments
    ///
    /// * `to` - Recipient address.
    /// * `amount` - Amount to mint.
    fn mint(&self, to: &Address, amount: i128) -> EffectResult<()>;

    /// Burn tokens from an address.
    ///
    /// # Arguments
    ///
    /// * `from` - Address to burn from.
    /// * `amount` - Amount to burn.
    fn burn(&self, from: &Address, amount: i128) -> EffectResult<()>;

    /// Transfer tokens between addresses.
    ///
    /// # Arguments
    ///
    /// * `from` - Source address.
    /// * `to` - Destination address.
    /// * `amount` - Amount to transfer.
    fn transfer(&self, from: &Address, to: &Address, amount: i128) -> EffectResult<()>;

    /// Get balance of an address.
    ///
    /// # Arguments
    ///
    /// * `addr` - Address to query.
    ///
    /// # Returns
    ///
    /// The token balance.
    fn balance(&self, addr: &Address) -> EffectResult<i128>;
}

// ---------------------------------------------------------------------------
// SDK Token Adapter
// ---------------------------------------------------------------------------

/// SEP-41 token adapter using the Soroban SDK's StellarAssetClient.
///
/// This adapter wraps a `StellarAssetClient` and implements the `Sep41Token` trait
/// for interacting with SEP-41 compliant token contracts. It supports both
/// standard operations (transfer, burn, balance) and admin operations (mint).
pub struct SdkTokenAdapter<'a> {
    client: StellarAssetClient<'a>,
}

impl<'a> SdkTokenAdapter<'a> {
    /// Create a new SDK token adapter.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment.
    /// * `contract_id` - The token contract address.
    #[inline]
    #[must_use]
    pub fn new(env: &'a Env, contract_id: &Address) -> Self {
        Self {
            client: StellarAssetClient::new(env, contract_id),
        }
    }
}

impl Sep41Token for SdkTokenAdapter<'_> {
    fn mint(&self, to: &Address, amount: i128) -> EffectResult<()> {
        self.client.mint(to, &amount);
        Ok(())
    }

    fn burn(&self, from: &Address, amount: i128) -> EffectResult<()> {
        self.client.burn(from, &amount);
        Ok(())
    }

    fn transfer(&self, from: &Address, to: &Address, amount: i128) -> EffectResult<()> {
        self.client.transfer(from, to, &amount);
        Ok(())
    }

    fn balance(&self, addr: &Address) -> EffectResult<i128> {
        Ok(self.client.balance(addr))
    }
}

// ---------------------------------------------------------------------------
// Test Token Adapter
// ---------------------------------------------------------------------------

/// Test SEP-41 token for use with soroban-sdk testutils.
///
/// Records operations without actually performing them.
#[derive(Clone, Debug, Default)]
pub struct TestSep41Token {
    /// Whether operations should fail.
    pub should_fail: bool,
    /// Mock balance to return.
    pub mock_balance: i128,
}

/// A recorded SEP-41 operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sep41Operation {
    /// Mint operation.
    Mint { to: Address, amount: i128 },
    /// Burn operation.
    Burn { from: Address, amount: i128 },
    /// Transfer operation.
    Transfer {
        from: Address,
        to: Address,
        amount: i128,
    },
    /// Balance query.
    Balance { addr: Address },
}

impl TestSep41Token {
    /// Create a new test token.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            should_fail: false,
            mock_balance: 1000,
        }
    }

    /// Create a test token that fails all operations.
    #[inline]
    #[must_use]
    pub fn failing() -> Self {
        Self {
            should_fail: true,
            mock_balance: 0,
        }
    }
}

impl Sep41Token for TestSep41Token {
    fn mint(&self, _to: &Address, _amount: i128) -> EffectResult<()> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test mint failed"));
        }
        Ok(())
    }

    fn burn(&self, _from: &Address, _amount: i128) -> EffectResult<()> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test burn failed"));
        }
        Ok(())
    }

    fn transfer(&self, _from: &Address, _to: &Address, _amount: i128) -> EffectResult<()> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test transfer failed"));
        }
        Ok(())
    }

    fn balance(&self, _addr: &Address) -> EffectResult<i128> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test balance failed"));
        }
        Ok(self.mock_balance)
    }
}

// ---------------------------------------------------------------------------
// Mock Interpreter (for testing)
// ---------------------------------------------------------------------------

/// A mock effect interpreter for testing that records effects without executing them.
#[derive(Clone, Debug, Default)]
pub struct MockInterpreter {
    /// Whether operations should fail.
    pub should_fail: bool,
    /// Recorded effects for test inspection.
    pub effects: Vec<KernelEffect>,
}

impl MockInterpreter {
    /// Create a new mock interpreter.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            should_fail: false,
            effects: Vec::new(),
        }
    }

    /// Create a failing mock interpreter.
    #[inline]
    #[must_use]
    pub fn failing() -> Self {
        Self {
            should_fail: true,
            effects: Vec::new(),
        }
    }

    /// Clear recorded effects.
    #[inline]
    pub fn clear(&mut self) {
        self.effects.clear();
    }
}

impl EffectInterpreter for MockInterpreter {
    fn execute_effect(&mut self, effect: &KernelEffect, _ctx: &EffectContext) -> EffectResult<()> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("mock interpreter failed"));
        }
        self.effects.push(effect.clone());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Address Mapping
// ---------------------------------------------------------------------------

/// Mapping from kernel addresses to Soroban addresses.
///
/// The kernel uses 32-byte arrays for addresses, while Soroban uses
/// opaque `Address` types. This struct provides the mapping for
/// address resolution during effect execution.
pub struct AddressMap<'a> {
    env: &'a Env,
    /// Map of kernel address bytes to Soroban addresses.
    addresses: alloc::collections::BTreeMap<[u8; 32], Address>,
}

impl<'a> AddressMap<'a> {
    /// Create a new address map.
    #[inline]
    #[must_use]
    pub fn new(env: &'a Env) -> Self {
        Self {
            env,
            addresses: alloc::collections::BTreeMap::new(),
        }
    }

    /// Register a kernel address with its corresponding Soroban address.
    #[inline]
    pub fn register(&mut self, kernel_addr: [u8; 32], soroban_addr: Address) {
        self.addresses.insert(kernel_addr, soroban_addr);
    }

    /// Resolve a kernel address to a Soroban address.
    ///
    /// Returns `None` if the address is not registered.
    #[inline]
    #[must_use]
    pub fn resolve(&self, kernel_addr: &[u8; 32]) -> Option<&Address> {
        self.addresses.get(kernel_addr)
    }
}

// ---------------------------------------------------------------------------
// Soroban Effect Interpreter
// ---------------------------------------------------------------------------

/// Effect interpreter for Soroban that executes effects via SEP-41 tokens.
///
/// This interpreter handles kernel effects by calling the appropriate
/// SEP-41 token operations for minting, burning, and transferring.
pub struct SorobanEffectInterpreter<'a, S, A>
where
    S: Sep41Token,
    A: Sep41Token,
{
    /// Reference to the Soroban environment.
    pub env: &'a Env,
    /// Share token contract interface.
    pub share_token: &'a S,
    /// Asset token contract interface.
    pub asset_token: &'a A,
    /// Address mapping from kernel to Soroban addresses.
    pub address_map: AddressMap<'a>,
    /// Recorded events.
    pub events: Vec<templar_vault_kernel::effects::KernelEvent>,
}

impl<'a, S, A> SorobanEffectInterpreter<'a, S, A>
where
    S: Sep41Token,
    A: Sep41Token,
{
    /// Create a new Soroban effect interpreter.
    #[inline]
    #[must_use]
    pub fn new(env: &'a Env, share_token: &'a S, asset_token: &'a A) -> Self {
        Self {
            env,
            share_token,
            asset_token,
            address_map: AddressMap::new(env),
            events: Vec::new(),
        }
    }

    /// Register an address mapping.
    #[inline]
    pub fn register_address(&mut self, kernel_addr: [u8; 32], soroban_addr: Address) {
        self.address_map.register(kernel_addr, soroban_addr);
    }

    /// Convert u128 to i128 safely for SEP-41 calls.
    ///
    /// SEP-41 uses i128 for amounts. This conversion fails if the value
    /// exceeds i128::MAX.
    #[inline]
    fn u128_to_i128(amount: u128) -> EffectResult<i128> {
        i128::try_from(amount)
            .map_err(|_| RuntimeError::effect_failed("amount overflow converting to i128"))
    }

    /// Resolve a kernel address to a Soroban address.
    fn resolve_address(&self, kernel_addr: &[u8; 32]) -> EffectResult<&Address> {
        self.address_map
            .resolve(kernel_addr)
            .ok_or_else(|| RuntimeError::effect_failed("unknown address"))
    }

    /// Convert u128 to i128 for event fields.
    #[inline]
    fn u128_to_i128_event(val: u128) -> i128 {
        // Saturate at i128::MAX for event data
        i128::try_from(val).unwrap_or(i128::MAX)
    }

    /// Emit a kernel event to the Soroban ledger using `#[contractevent]` structs.
    fn emit_event(
        &self,
        event: &templar_vault_kernel::effects::KernelEvent,
    ) -> EffectResult<()> {
        use templar_vault_kernel::effects::KernelEvent;

        match event {
            KernelEvent::DepositProcessed {
                owner,
                receiver,
                assets_in,
                shares_out,
            } => {
                let owner_addr = self.address_map.resolve(owner);
                let recv_addr = self.address_map.resolve(receiver);
                if let (Some(o), Some(r)) = (owner_addr, recv_addr) {
                    DepositEvent {
                        owner: o.clone(),
                        receiver: r.clone(),
                        assets_in: Self::u128_to_i128_event(*assets_in),
                        shares_out: Self::u128_to_i128_event(*shares_out),
                    }
                    .publish(self.env);
                }
            }
            KernelEvent::WithdrawalRequested {
                id,
                owner,
                receiver,
                shares,
                expected_assets,
            } => {
                let owner_addr = self.address_map.resolve(owner);
                let recv_addr = self.address_map.resolve(receiver);
                if let (Some(o), Some(r)) = (owner_addr, recv_addr) {
                    WithdrawRequestEvent {
                        id: *id,
                        owner: o.clone(),
                        receiver: r.clone(),
                        shares: Self::u128_to_i128_event(*shares),
                        expected_assets: Self::u128_to_i128_event(*expected_assets),
                    }
                    .publish(self.env);
                }
            }
            KernelEvent::WithdrawalStarted {
                op_id,
                amount,
                escrow_shares,
                owner,
                receiver,
            } => {
                let owner_addr = self.address_map.resolve(owner);
                let recv_addr = self.address_map.resolve(receiver);
                if let (Some(o), Some(r)) = (owner_addr, recv_addr) {
                    WithdrawStartEvent {
                        op_id: *op_id,
                        amount: Self::u128_to_i128_event(*amount),
                        escrow_shares: Self::u128_to_i128_event(*escrow_shares),
                        owner: o.clone(),
                        receiver: r.clone(),
                    }
                    .publish(self.env);
                }
            }
            KernelEvent::WithdrawalCollected {
                op_id,
                burn_shares,
                collected,
            } => {
                WithdrawCollectedEvent {
                    op_id: *op_id,
                    burn_shares: Self::u128_to_i128_event(*burn_shares),
                    collected: Self::u128_to_i128_event(*collected),
                }
                .publish(self.env);
            }
            KernelEvent::WithdrawalStopped {
                op_id,
                escrow_shares,
            } => {
                WithdrawStoppedEvent {
                    op_id: *op_id,
                    escrow_shares: Self::u128_to_i128_event(*escrow_shares),
                }
                .publish(self.env);
            }
            KernelEvent::PayoutCompleted {
                op_id,
                success,
                burn_shares,
                refund_shares,
                amount,
            } => {
                PayoutEvent {
                    op_id: *op_id,
                    success: *success,
                    burn_shares: Self::u128_to_i128_event(*burn_shares),
                    refund_shares: Self::u128_to_i128_event(*refund_shares),
                    amount: Self::u128_to_i128_event(*amount),
                }
                .publish(self.env);
            }
            KernelEvent::AllocationStarted { op_id, total, plan_len } => {
                AllocStartEvent {
                    op_id: *op_id,
                    total: Self::u128_to_i128_event(*total),
                    plan_len: *plan_len,
                }
                .publish(self.env);
            }
            KernelEvent::AllocationStepFailed {
                op_id,
                index,
                remaining,
            } => {
                AllocStepFailEvent {
                    op_id: *op_id,
                    index: *index,
                    remaining: Self::u128_to_i128_event(*remaining),
                }
                .publish(self.env);
            }
            KernelEvent::AllocationCompleted {
                op_id,
                has_withdrawal,
            } => {
                AllocDoneEvent {
                    op_id: *op_id,
                    has_withdrawal: *has_withdrawal,
                }
                .publish(self.env);
            }
            KernelEvent::RefreshStarted { op_id, plan_len } => {
                RefreshStartEvent {
                    op_id: *op_id,
                    plan_len: *plan_len,
                }
                .publish(self.env);
            }
            KernelEvent::RefreshCompleted { op_id } => {
                RefreshDoneEvent { op_id: *op_id }.publish(self.env);
            }
            KernelEvent::ExternalAssetsSynced {
                op_id,
                new_external_assets,
                total_assets,
            } => {
                ExtAssetsSyncEvent {
                    op_id: *op_id,
                    new_external_assets: Self::u128_to_i128_event(*new_external_assets),
                    total_assets: Self::u128_to_i128_event(*total_assets),
                }
                .publish(self.env);
            }
            KernelEvent::FeesRefreshed {
                now_ns,
                total_assets,
            } => {
                FeesRefreshEvent {
                    now_ns: *now_ns,
                    total_assets: Self::u128_to_i128_event(*total_assets),
                }
                .publish(self.env);
            }
            KernelEvent::PauseUpdated { paused } => {
                PauseUpdatedEvent { paused: *paused }.publish(self.env);
            }
        }

        Ok(())
    }
}

impl<S, A> EffectInterpreter for SorobanEffectInterpreter<'_, S, A>
where
    S: Sep41Token,
    A: Sep41Token,
{
    fn execute_effect(&mut self, effect: &KernelEffect, ctx: &EffectContext) -> EffectResult<()> {
        match effect {
            KernelEffect::MintShares { owner, shares } => {
                let amount = Self::u128_to_i128(*shares)?;
                let addr = self.resolve_address(owner)?;
                self.share_token.mint(addr, amount)
            }

            KernelEffect::BurnShares { owner, shares } => {
                let amount = Self::u128_to_i128(*shares)?;
                let addr = self.resolve_address(owner)?;
                self.share_token.burn(addr, amount)
            }

            KernelEffect::TransferShares { from, to, shares } => {
                let amount = Self::u128_to_i128(*shares)?;
                let from_addr = self.resolve_address(from)?;
                let to_addr = self.resolve_address(to)?;
                self.share_token.transfer(from_addr, to_addr, amount)
            }

            KernelEffect::TransferAssets { to, amount } => {
                let amount_i128 = Self::u128_to_i128(*amount)?;
                let to_addr = self.resolve_address(to)?;
                let vault_addr = self.resolve_address(&ctx.vault_address)?;
                // Transfer from vault to recipient
                self.asset_token.transfer(vault_addr, to_addr, amount_i128)
            }

            KernelEffect::EmitEvent { event } => {
                self.emit_event(event)?;
                self.events.push(event.clone());
                Ok(())
            }

            // Chain-specific effects (NEAR only) - unreachable in Soroban
            #[allow(unreachable_patterns)]
            _ => Err(RuntimeError::effect_failed(
                "unsupported effect type for Soroban",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    fn test_env() -> Env {
        Env::default()
    }

    fn test_context() -> EffectContext {
        EffectContext::new(
            1_000_000_000_000,
            [1u8; 32],
            [2u8; 32],
            [3u8; 32],
        )
    }

    #[test]
    fn test_effect_summary_new() {
        let summary = EffectSummary::new();
        assert_eq!(summary.shares_minted, 0);
        assert_eq!(summary.shares_burned, 0);
        assert_eq!(summary.shares_transferred, 0);
        assert_eq!(summary.assets_transferred, 0);
        assert_eq!(summary.events_emitted, 0);
    }

    #[test]
    fn test_effect_summary_recording() {
        let mut summary = EffectSummary::new();

        summary.record_mint(100);
        assert_eq!(summary.shares_minted, 100);

        summary.record_burn(50);
        assert_eq!(summary.shares_burned, 50);

        summary.record_share_transfer(25);
        assert_eq!(summary.shares_transferred, 25);

        summary.record_asset_transfer(1000);
        assert_eq!(summary.assets_transferred, 1000);

        summary.record_event();
        summary.record_event();
        assert_eq!(summary.events_emitted, 2);
    }

    #[test]
    fn test_effect_context_new() {
        let ctx = test_context();
        assert_eq!(ctx.now_ns, 1_000_000_000_000);
        assert_eq!(ctx.vault_address, [1u8; 32]);
        assert_eq!(ctx.asset_address, [2u8; 32]);
        assert_eq!(ctx.share_address, [3u8; 32]);
    }

    #[test]
    fn test_test_sep41_token_mint() {
        let env = test_env();
        let token = TestSep41Token::new();
        let addr = Address::generate(&env);
        let result = token.mint(&addr, 100);
        assert!(result.is_ok());
    }

    #[test]
    fn test_test_sep41_token_burn() {
        let env = test_env();
        let token = TestSep41Token::new();
        let addr = Address::generate(&env);
        let result = token.burn(&addr, 50);
        assert!(result.is_ok());
    }

    #[test]
    fn test_test_sep41_token_transfer() {
        let env = test_env();
        let token = TestSep41Token::new();
        let from = Address::generate(&env);
        let to = Address::generate(&env);
        let result = token.transfer(&from, &to, 25);
        assert!(result.is_ok());
    }

    #[test]
    fn test_test_sep41_token_balance() {
        let env = test_env();
        let token = TestSep41Token::new();
        let addr = Address::generate(&env);
        let result = token.balance(&addr);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1000);
    }

    #[test]
    fn test_test_sep41_token_failing() {
        let env = test_env();
        let token = TestSep41Token::failing();
        let addr = Address::generate(&env);
        let from = Address::generate(&env);
        let to = Address::generate(&env);

        assert!(token.mint(&addr, 100).is_err());
        assert!(token.burn(&addr, 50).is_err());
        assert!(token.transfer(&from, &to, 25).is_err());
        assert!(token.balance(&addr).is_err());
    }

    #[test]
    fn test_u128_to_i128_conversion() {
        // Valid conversions
        assert!(SorobanEffectInterpreter::<TestSep41Token, TestSep41Token>::u128_to_i128(0).is_ok());
        assert!(SorobanEffectInterpreter::<TestSep41Token, TestSep41Token>::u128_to_i128(1000)
            .is_ok());
        assert!(SorobanEffectInterpreter::<TestSep41Token, TestSep41Token>::u128_to_i128(
            i128::MAX as u128
        )
        .is_ok());

        // Overflow
        assert!(SorobanEffectInterpreter::<TestSep41Token, TestSep41Token>::u128_to_i128(
            (i128::MAX as u128) + 1
        )
        .is_err());
    }

    #[test]
    fn test_address_map() {
        let env = test_env();
        let mut map = AddressMap::new(&env);

        let kernel_addr = [1u8; 32];
        let soroban_addr = Address::generate(&env);

        map.register(kernel_addr, soroban_addr.clone());

        let resolved = map.resolve(&kernel_addr);
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap(), &soroban_addr);

        // Unknown address
        let unknown = [2u8; 32];
        assert!(map.resolve(&unknown).is_none());
    }
}
