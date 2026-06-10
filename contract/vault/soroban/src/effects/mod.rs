//! Effect interpreter for kernel effects on Soroban.

use soroban_sdk::{contractevent, token::StellarAssetClient, Address, BytesN, Env};
use templar_vault_kernel::effects::{KernelEffect, KernelEvent, WithdrawalSkipReason};
use templar_vault_kernel::{AddressBook, TimestampNs};

use crate::convert::u128_to_i128_effect;
use crate::error::RuntimeError;

/// Short helper to convert u128 to i128 for event / effect amounts.
#[inline]
pub(crate) fn to_i128_event(value: u128) -> Result<i128, RuntimeError> {
    u128_to_i128_effect(value, "event amount overflow")
}

#[inline(never)]
fn event_address(env: &Env, value: &templar_vault_kernel::Address) -> BytesN<32> {
    BytesN::from_array(env, value.as_bytes())
}

#[contractevent]
#[derive(Clone)]
pub struct AllocationStartedEvent {
    pub op_id: u64,
    pub total: u128,
    pub plan_len: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct AllocationStepFailedEvent {
    pub op_id: u64,
    pub index: u32,
    pub remaining: u128,
    pub total_allocated: u128,
}

#[contractevent]
#[derive(Clone)]
pub struct AllocationCompletedEvent {
    pub op_id: u64,
    pub has_withdrawal: bool,
}

#[contractevent]
#[derive(Clone)]
pub struct WithdrawalStartedEvent {
    pub op_id: u64,
    pub amount: u128,
    pub escrow_shares: u128,
    pub owner: BytesN<32>,
    pub receiver: BytesN<32>,
}

#[contractevent]
#[derive(Clone)]
pub struct WithdrawalCollectedEvent {
    pub op_id: u64,
    pub burn_shares: u128,
    pub collected: u128,
}

#[contractevent]
#[derive(Clone)]
pub struct WithdrawalStoppedEvent {
    pub op_id: u64,
    pub escrow_shares: u128,
}

#[contractevent]
#[derive(Clone)]
pub struct WithdrawalSkippedEvent {
    pub id: u64,
    pub owner: BytesN<32>,
    pub receiver: BytesN<32>,
    pub escrow_shares: u128,
    pub expected_assets: u128,
    pub reason: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct RefreshStartedEvent {
    pub op_id: u64,
    pub plan_len: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct RefreshCompletedEvent {
    pub op_id: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct PayoutCompletedEvent {
    pub op_id: u64,
    pub success: bool,
    pub burn_shares: u128,
    pub refund_shares: u128,
    pub amount: u128,
}

#[contractevent]
#[derive(Clone)]
pub struct DepositProcessedEvent {
    pub owner: BytesN<32>,
    pub receiver: BytesN<32>,
    pub assets_in: u128,
    pub shares_out: u128,
}

#[contractevent]
#[derive(Clone)]
pub struct AtomicWithdrawProcessedEvent {
    pub owner: BytesN<32>,
    pub receiver: BytesN<32>,
    pub shares_burned: u128,
    pub assets_out: u128,
}

#[contractevent]
#[derive(Clone)]
pub struct WithdrawalRequestedEvent {
    pub id: u64,
    pub owner: BytesN<32>,
    pub receiver: BytesN<32>,
    pub shares: u128,
    pub expected_assets: u128,
}

#[contractevent]
#[derive(Clone)]
pub struct ExternalAssetsSyncedEvent {
    pub op_id: u64,
    pub new_external_assets: u128,
    pub total_assets: u128,
}

#[contractevent]
#[derive(Clone)]
pub struct FeesRefreshedEvent {
    pub now_ns: u64,
    pub total_assets: u128,
}

#[contractevent]
#[derive(Clone)]
pub struct PauseUpdatedEvent {
    pub paused: bool,
}

#[contractevent]
#[derive(Clone)]
pub struct EmergencyResetCompletedEvent {
    pub op_id: u64,
    pub from_state: u32,
}

/// Publish a KernelEvent as typed Soroban contract events.
#[inline(never)]
pub fn publish_kernel_event(env: &Env, event: &KernelEvent) -> Result<(), RuntimeError> {
    match event {
        KernelEvent::AllocationStarted {
            op_id,
            total,
            plan_len,
        } => AllocationStartedEvent {
            op_id: *op_id,
            total: *total,
            plan_len: *plan_len,
        }
        .publish(env),
        KernelEvent::AllocationStepFailed {
            op_id,
            index,
            remaining,
            total_allocated,
        } => AllocationStepFailedEvent {
            op_id: *op_id,
            index: *index,
            remaining: *remaining,
            total_allocated: *total_allocated,
        }
        .publish(env),
        KernelEvent::AllocationCompleted {
            op_id,
            has_withdrawal,
        } => AllocationCompletedEvent {
            op_id: *op_id,
            has_withdrawal: *has_withdrawal,
        }
        .publish(env),
        KernelEvent::WithdrawalStarted {
            op_id,
            amount,
            escrow_shares,
            owner,
            receiver,
        } => WithdrawalStartedEvent {
            op_id: *op_id,
            amount: *amount,
            escrow_shares: *escrow_shares,
            owner: event_address(env, owner),
            receiver: event_address(env, receiver),
        }
        .publish(env),
        KernelEvent::WithdrawalCollected {
            op_id,
            burn_shares,
            collected,
        } => WithdrawalCollectedEvent {
            op_id: *op_id,
            burn_shares: *burn_shares,
            collected: *collected,
        }
        .publish(env),
        KernelEvent::WithdrawalStopped {
            op_id,
            escrow_shares,
        } => WithdrawalStoppedEvent {
            op_id: *op_id,
            escrow_shares: *escrow_shares,
        }
        .publish(env),
        KernelEvent::WithdrawalSkipped {
            id,
            owner,
            receiver,
            escrow_shares,
            expected_assets,
            reason,
        } => WithdrawalSkippedEvent {
            id: *id,
            owner: event_address(env, owner),
            receiver: event_address(env, receiver),
            escrow_shares: *escrow_shares,
            expected_assets: *expected_assets,
            reason: match reason {
                WithdrawalSkipReason::ZeroExpectedAssets => 0,
                WithdrawalSkipReason::Restricted => 1,
            },
        }
        .publish(env),
        KernelEvent::RefreshStarted { op_id, plan_len } => RefreshStartedEvent {
            op_id: *op_id,
            plan_len: *plan_len,
        }
        .publish(env),
        KernelEvent::RefreshCompleted { op_id } => {
            RefreshCompletedEvent { op_id: *op_id }.publish(env)
        }
        KernelEvent::PayoutCompleted {
            op_id,
            success,
            burn_shares,
            refund_shares,
            amount,
        } => PayoutCompletedEvent {
            op_id: *op_id,
            success: *success,
            burn_shares: *burn_shares,
            refund_shares: *refund_shares,
            amount: *amount,
        }
        .publish(env),
        KernelEvent::DepositProcessed {
            owner,
            receiver,
            assets_in,
            shares_out,
        } => DepositProcessedEvent {
            owner: event_address(env, owner),
            receiver: event_address(env, receiver),
            assets_in: *assets_in,
            shares_out: *shares_out,
        }
        .publish(env),
        KernelEvent::AtomicWithdrawProcessed {
            owner,
            receiver,
            shares_burned,
            assets_out,
        } => AtomicWithdrawProcessedEvent {
            owner: event_address(env, owner),
            receiver: event_address(env, receiver),
            shares_burned: *shares_burned,
            assets_out: *assets_out,
        }
        .publish(env),
        KernelEvent::WithdrawalRequested {
            id,
            owner,
            receiver,
            shares,
            expected_assets,
        } => WithdrawalRequestedEvent {
            id: *id,
            owner: event_address(env, owner),
            receiver: event_address(env, receiver),
            shares: *shares,
            expected_assets: *expected_assets,
        }
        .publish(env),
        KernelEvent::ExternalAssetsSynced {
            op_id,
            new_external_assets,
            total_assets,
        } => ExternalAssetsSyncedEvent {
            op_id: *op_id,
            new_external_assets: *new_external_assets,
            total_assets: *total_assets,
        }
        .publish(env),
        KernelEvent::FeesRefreshed {
            now_ns,
            total_assets,
        } => FeesRefreshedEvent {
            now_ns: *now_ns,
            total_assets: *total_assets,
        }
        .publish(env),
        KernelEvent::PauseUpdated { paused } => PauseUpdatedEvent { paused: *paused }.publish(env),
        KernelEvent::EmergencyResetCompleted { op_id, from_state } => {
            EmergencyResetCompletedEvent {
                op_id: *op_id,
                from_state: *from_state,
            }
            .publish(env)
        }
    }
    Ok(())
}

/// Result type for effect operations.
pub type EffectResult<T> = Result<T, RuntimeError>;

/// Context provided to effect handlers.
///
/// Contains information about the current execution environment
/// that effect handlers may need.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone)]
pub struct EffectContext {
    /// Current timestamp in nanoseconds.
    pub now_ns: TimestampNs,
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
            now_ns: TimestampNs(now_ns),
            vault_address,
            asset_address,
            share_address,
        }
    }
}

/// Effect execution summary.
///
/// Tracks the cumulative results of executing a batch of effects.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Default, PartialEq, Eq)]
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

    /// Merge another summary into this one.
    #[inline]
    pub fn merge(&mut self, other: EffectSummary) {
        self.shares_minted = self.shares_minted.saturating_add(other.shares_minted);
        self.shares_burned = self.shares_burned.saturating_add(other.shares_burned);
        self.shares_transferred = self
            .shares_transferred
            .saturating_add(other.shares_transferred);
        self.assets_transferred = self
            .assets_transferred
            .saturating_add(other.assets_transferred);
        self.events_emitted = self.events_emitted.saturating_add(other.events_emitted);
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
                KernelEffect::BurnSharesFrom { shares, .. } => {
                    summary.record_burn(*shares);
                }
                KernelEffect::TransferShares { shares, .. } => {
                    summary.record_share_transfer(*shares);
                }
                KernelEffect::TransferAssets { amount, .. }
                | KernelEffect::TransferAssetsFrom { amount, .. } => {
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

/// Address mapping support for effect interpreters.
pub trait AddressRegistrar {
    /// Register a kernel address with its corresponding Soroban address.
    fn register_address(
        &mut self,
        kernel_addr: templar_vault_kernel::Address,
        soroban_addr: Address,
    );

    /// Return true if the kernel address is registered.
    fn has_address(&self, kernel_addr: &templar_vault_kernel::Address) -> bool;
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

    fn burn_from(&self, spender: &Address, from: &Address, amount: i128) -> EffectResult<()>;

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
/// standard operations (transfer, burn, balance) and privileged operations (mint).
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

    fn burn_from(&self, spender: &Address, from: &Address, amount: i128) -> EffectResult<()> {
        self.client.burn_from(spender, from, &amount);
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

pub type ShareTokenAdapter<'a> = SdkTokenAdapter<'a>;

// ---------------------------------------------------------------------------
// Address Mapping
// ---------------------------------------------------------------------------

/// Mapping from kernel addresses to Soroban addresses.
///
/// The kernel uses 32-byte arrays for addresses, while Soroban uses
/// opaque `Address` types. This struct provides the mapping for
/// address resolution during effect execution.
///
/// Note: the map is expected to stay small (vault + escrow + a few
/// participant addresses per call). If this ever grows large, consider
/// a fixed-capacity array or Vec-based linear scan to reduce overhead.
pub struct AddressMap {
    /// Map of kernel address bytes to Soroban addresses.
    addresses: AddressBook<Address>,
}

impl AddressMap {
    /// Create a new address map.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            addresses: AddressBook::new(),
        }
    }

    /// Register a kernel address with its corresponding Soroban address.
    #[inline]
    pub fn register(&mut self, kernel_addr: templar_vault_kernel::Address, soroban_addr: Address) {
        self.addresses.insert(kernel_addr, soroban_addr);
    }

    /// Resolve a kernel address to a Soroban address.
    ///
    /// Returns `None` if the address is not registered.
    #[inline]
    #[must_use]
    pub fn resolve(&self, kernel_addr: &templar_vault_kernel::Address) -> Option<&Address> {
        self.addresses.resolve(kernel_addr)
    }
}

impl Default for AddressMap {
    fn default() -> Self {
        Self::new()
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
    pub address_map: AddressMap,
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
            address_map: AddressMap::new(),
        }
    }

    /// Register an address mapping.
    #[inline]
    pub fn register_address(
        &mut self,
        kernel_addr: templar_vault_kernel::Address,
        soroban_addr: Address,
    ) {
        self.address_map.register(kernel_addr, soroban_addr);
    }

    /// Resolve a kernel address to a Soroban address.
    fn resolve_address(
        &self,
        kernel_addr: &templar_vault_kernel::Address,
    ) -> EffectResult<&Address> {
        match self.address_map.resolve(kernel_addr) {
            Some(address) => Ok(address),
            None => Err(RuntimeError::effect_failed("unknown address")),
        }
    }
}

impl<S, A> AddressRegistrar for SorobanEffectInterpreter<'_, S, A>
where
    S: Sep41Token,
    A: Sep41Token,
{
    fn register_address(
        &mut self,
        kernel_addr: templar_vault_kernel::Address,
        soroban_addr: Address,
    ) {
        self.address_map.register(kernel_addr, soroban_addr);
    }

    fn has_address(&self, kernel_addr: &templar_vault_kernel::Address) -> bool {
        self.address_map.resolve(kernel_addr).is_some()
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
                let amount = to_i128_event(*shares)?;
                let addr = self.resolve_address(owner)?;
                self.share_token.mint(addr, amount)
            }

            KernelEffect::BurnShares { owner, shares } => {
                let amount = to_i128_event(*shares)?;
                let addr = self.resolve_address(owner)?;
                self.share_token.burn(addr, amount)
            }

            KernelEffect::BurnSharesFrom {
                spender,
                owner,
                shares,
            } => {
                let amount = to_i128_event(*shares)?;
                let spender_addr = self.resolve_address(spender)?;
                let owner_addr = self.resolve_address(owner)?;
                self.share_token.burn_from(spender_addr, owner_addr, amount)
            }

            KernelEffect::TransferShares { from, to, shares } => {
                let amount = to_i128_event(*shares)?;
                let from_addr = self.resolve_address(from)?;
                let to_addr = self.resolve_address(to)?;
                self.share_token.transfer(from_addr, to_addr, amount)
            }

            KernelEffect::TransferAssets { to, amount } => {
                let amount_i128 = to_i128_event(*amount)?;
                let to_addr = self.resolve_address(to)?;
                let vault_addr = self.resolve_address(&ctx.vault_address)?;
                // Transfer from vault to recipient
                self.asset_token.transfer(vault_addr, to_addr, amount_i128)
            }

            KernelEffect::TransferAssetsFrom { from, to, amount } => {
                let amount_i128 = to_i128_event(*amount)?;
                let from_addr = self.resolve_address(from)?;
                let to_addr = self.resolve_address(to)?;
                self.asset_token.transfer(from_addr, to_addr, amount_i128)
            }

            KernelEffect::EmitEvent { event } => publish_kernel_event(self.env, event),

            // Chain-specific effects (NEAR only) - unreachable in Soroban
            #[allow(unreachable_patterns)]
            _ => Err(RuntimeError::effect_failed(
                "unsupported effect type for Soroban",
            )),
        }
    }
}
