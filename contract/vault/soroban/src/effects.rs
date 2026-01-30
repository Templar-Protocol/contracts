//! Effect interpreter for processing kernel effects on Soroban.
//!
//! This module provides the [`EffectInterpreter`] trait and supporting types
//! for executing kernel effects on the Soroban blockchain.

use alloc::vec::Vec;
use templar_vault_kernel::{effects::KernelEffect, Address};

use crate::error::RuntimeError;

/// Result type for effect operations.
pub type EffectResult<T> = Result<T, RuntimeError>;

/// Context provided to effect handlers.
///
/// Contains information about the current execution environment
/// that effect handlers may need.
#[derive(Clone, Debug)]
pub struct EffectContext {
    /// Current timestamp in nanoseconds.
    pub now_ns: u64,
    /// The vault contract address.
    pub vault_address: Address,
    /// The underlying asset contract address (for SEP-41 transfers).
    pub asset_address: Address,
    /// The share token contract address.
    pub share_address: Address,
}

impl EffectContext {
    /// Create a new effect context.
    #[inline]
    #[must_use]
    pub const fn new(
        now_ns: u64,
        vault_address: Address,
        asset_address: Address,
        share_address: Address,
    ) -> Self {
        Self {
            now_ns,
            vault_address,
            asset_address,
            share_address,
        }
    }
}

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
    /// `Ok(EffectSummary)` with execution statistics, or `Err(RuntimeError)` on failure.
    fn execute_effects(
        &mut self,
        effects: &[KernelEffect],
        ctx: &EffectContext,
    ) -> EffectResult<EffectSummary> {
        let mut summary = EffectSummary::new();

        for effect in effects {
            self.execute_effect(effect, ctx)?;

            match effect {
                KernelEffect::MintShares { shares, .. } => summary.record_mint(*shares),
                KernelEffect::BurnShares { shares, .. } => summary.record_burn(*shares),
                KernelEffect::TransferShares { shares, .. } => {
                    summary.record_share_transfer(*shares)
                }
                KernelEffect::TransferAssets { amount, .. } => {
                    summary.record_asset_transfer(*amount)
                }
                KernelEffect::EmitEvent { .. } => summary.record_event(),
                // Chain-specific effects (NEAR only) - unreachable in Soroban context
                #[allow(unreachable_patterns)]
                _ => {}
            }
        }

        Ok(summary)
    }
}

/// A mock effect interpreter for testing.
///
/// Records all executed effects without actually performing them.
#[derive(Clone, Debug, Default)]
pub struct MockInterpreter {
    /// Recorded effects.
    pub effects: Vec<KernelEffect>,
    /// Whether to fail on execution.
    pub should_fail: bool,
    /// Failure message.
    pub failure_msg: Option<&'static str>,
}

impl MockInterpreter {
    /// Create a new mock interpreter.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a mock interpreter that fails all executions.
    #[inline]
    #[must_use]
    pub fn failing(msg: &'static str) -> Self {
        Self {
            effects: Vec::new(),
            should_fail: true,
            failure_msg: Some(msg),
        }
    }

    /// Get the number of recorded effects.
    #[inline]
    #[must_use]
    pub fn effect_count(&self) -> usize {
        self.effects.len()
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
            return Err(RuntimeError::effect_failed(
                self.failure_msg.unwrap_or("mock failure"),
            ));
        }
        self.effects.push(effect.clone());
        Ok(())
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
/// SEP-41 uses i128 for amounts, so we convert from u128 internally.
pub trait Sep41Token {
    /// Mint tokens to an address.
    ///
    /// # Arguments
    ///
    /// * `to` - Recipient address.
    /// * `amount` - Amount to mint.
    fn mint(&self, to: Address, amount: i128) -> EffectResult<()>;

    /// Burn tokens from an address.
    ///
    /// # Arguments
    ///
    /// * `from` - Address to burn from.
    /// * `amount` - Amount to burn.
    fn burn(&self, from: Address, amount: i128) -> EffectResult<()>;

    /// Transfer tokens between addresses.
    ///
    /// # Arguments
    ///
    /// * `from` - Source address.
    /// * `to` - Destination address.
    /// * `amount` - Amount to transfer.
    fn transfer(&self, from: Address, to: Address, amount: i128) -> EffectResult<()>;

    /// Get balance of an address.
    ///
    /// # Arguments
    ///
    /// * `addr` - Address to query.
    ///
    /// # Returns
    ///
    /// The token balance.
    fn balance(&self, addr: Address) -> EffectResult<i128>;
}

/// Mock SEP-41 token for testing.
///
/// Records all operations without actually performing them.
#[derive(Clone, Debug, Default)]
pub struct MockSep41Token {
    /// Whether operations should fail.
    pub should_fail: bool,
    /// Recorded operations.
    pub operations: Vec<Sep41Operation>,
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

impl MockSep41Token {
    /// Create a new mock token.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a mock token that fails all operations.
    #[inline]
    #[must_use]
    pub fn failing() -> Self {
        Self {
            should_fail: true,
            operations: Vec::new(),
        }
    }
}

impl Sep41Token for MockSep41Token {
    fn mint(&self, to: Address, amount: i128) -> EffectResult<()> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("mock mint failed"));
        }
        // Note: We can't mutate self in the trait method with &self
        // In real usage, this would be a contract call
        let _ = Sep41Operation::Mint { to, amount };
        Ok(())
    }

    fn burn(&self, from: Address, amount: i128) -> EffectResult<()> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("mock burn failed"));
        }
        let _ = Sep41Operation::Burn { from, amount };
        Ok(())
    }

    fn transfer(&self, from: Address, to: Address, amount: i128) -> EffectResult<()> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("mock transfer failed"));
        }
        let _ = Sep41Operation::Transfer { from, to, amount };
        Ok(())
    }

    fn balance(&self, addr: Address) -> EffectResult<i128> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("mock balance failed"));
        }
        let _ = Sep41Operation::Balance { addr };
        Ok(1000) // Mock balance
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
    /// Share token contract interface.
    pub share_token: &'a S,
    /// Asset token contract interface.
    pub asset_token: &'a A,
    /// Recorded events (mock event emission).
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
    pub fn new(share_token: &'a S, asset_token: &'a A) -> Self {
        Self {
            share_token,
            asset_token,
            events: Vec::new(),
        }
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
                self.share_token.mint(*owner, amount)
            }

            KernelEffect::BurnShares { owner, shares } => {
                let amount = Self::u128_to_i128(*shares)?;
                self.share_token.burn(*owner, amount)
            }

            KernelEffect::TransferShares { from, to, shares } => {
                let amount = Self::u128_to_i128(*shares)?;
                self.share_token.transfer(*from, *to, amount)
            }

            KernelEffect::TransferAssets { to, amount } => {
                let amount_i128 = Self::u128_to_i128(*amount)?;
                // Transfer from vault to recipient
                self.asset_token.transfer(ctx.vault_address, *to, amount_i128)
            }

            KernelEffect::EmitEvent { event } => {
                // In real Soroban, this would emit a contract event
                // For now, we just record it
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
    use alloc::vec;
    use templar_vault_kernel::effects::KernelEvent;

    fn test_context() -> EffectContext {
        EffectContext::new(1_000_000_000_000, [1u8; 32], [2u8; 32], [3u8; 32])
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
    fn test_mock_interpreter_records_effects() {
        let mut interpreter = MockInterpreter::new();
        let ctx = test_context();

        let effect = KernelEffect::MintShares {
            owner: [0u8; 32],
            shares: 100,
        };

        assert!(interpreter.execute_effect(&effect, &ctx).is_ok());
        assert_eq!(interpreter.effect_count(), 1);
    }

    #[test]
    fn test_mock_interpreter_execute_batch() {
        let mut interpreter = MockInterpreter::new();
        let ctx = test_context();

        let effects = vec![
            KernelEffect::MintShares {
                owner: [0u8; 32],
                shares: 100,
            },
            KernelEffect::BurnShares {
                owner: [0u8; 32],
                shares: 50,
            },
            KernelEffect::TransferShares {
                from: [0u8; 32],
                to: [1u8; 32],
                shares: 25,
            },
            KernelEffect::TransferAssets {
                to: [2u8; 32],
                amount: 1000,
            },
            KernelEffect::EmitEvent {
                event: KernelEvent::RefreshCompleted { op_id: 1 },
            },
        ];

        let result = interpreter.execute_effects(&effects, &ctx);
        assert!(result.is_ok());

        let summary = result.unwrap();
        assert_eq!(summary.shares_minted, 100);
        assert_eq!(summary.shares_burned, 50);
        assert_eq!(summary.shares_transferred, 25);
        assert_eq!(summary.assets_transferred, 1000);
        assert_eq!(summary.events_emitted, 1);

        assert_eq!(interpreter.effect_count(), 5);
    }

    #[test]
    fn test_mock_interpreter_failing() {
        let mut interpreter = MockInterpreter::failing("test failure");
        let ctx = test_context();

        let effect = KernelEffect::MintShares {
            owner: [0u8; 32],
            shares: 100,
        };

        let result = interpreter.execute_effect(&effect, &ctx);
        assert!(result.is_err());
        assert!(matches!(result, Err(RuntimeError::EffectFailed(_))));
    }

    #[test]
    fn test_mock_interpreter_batch_stops_on_failure() {
        let mut interpreter = MockInterpreter::new();
        interpreter.should_fail = true;
        interpreter.failure_msg = Some("fail on second");
        let ctx = test_context();

        let effects = vec![KernelEffect::MintShares {
            owner: [0u8; 32],
            shares: 100,
        }];

        let result = interpreter.execute_effects(&effects, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_effect_context_new() {
        let ctx = EffectContext::new(123, [1u8; 32], [2u8; 32], [3u8; 32]);
        assert_eq!(ctx.now_ns, 123);
        assert_eq!(ctx.vault_address, [1u8; 32]);
        assert_eq!(ctx.asset_address, [2u8; 32]);
        assert_eq!(ctx.share_address, [3u8; 32]);
    }

    // =========================================================================
    // SEP-41 Token tests
    // =========================================================================

    #[test]
    fn test_mock_sep41_token_mint() {
        let token = MockSep41Token::new();
        let result = token.mint([1u8; 32], 100);
        assert!(result.is_ok());
    }

    #[test]
    fn test_mock_sep41_token_burn() {
        let token = MockSep41Token::new();
        let result = token.burn([1u8; 32], 50);
        assert!(result.is_ok());
    }

    #[test]
    fn test_mock_sep41_token_transfer() {
        let token = MockSep41Token::new();
        let result = token.transfer([1u8; 32], [2u8; 32], 25);
        assert!(result.is_ok());
    }

    #[test]
    fn test_mock_sep41_token_balance() {
        let token = MockSep41Token::new();
        let result = token.balance([1u8; 32]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1000);
    }

    #[test]
    fn test_mock_sep41_token_failing() {
        let token = MockSep41Token::failing();
        assert!(token.mint([1u8; 32], 100).is_err());
        assert!(token.burn([1u8; 32], 50).is_err());
        assert!(token.transfer([1u8; 32], [2u8; 32], 25).is_err());
        assert!(token.balance([1u8; 32]).is_err());
    }

    // =========================================================================
    // SorobanEffectInterpreter tests
    // =========================================================================

    #[test]
    fn test_soroban_interpreter_mint_shares() {
        let share_token = MockSep41Token::new();
        let asset_token = MockSep41Token::new();
        let mut interpreter = SorobanEffectInterpreter::new(&share_token, &asset_token);
        let ctx = test_context();

        let effect = KernelEffect::MintShares {
            owner: [10u8; 32],
            shares: 1000,
        };

        let result = interpreter.execute_effect(&effect, &ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_soroban_interpreter_burn_shares() {
        let share_token = MockSep41Token::new();
        let asset_token = MockSep41Token::new();
        let mut interpreter = SorobanEffectInterpreter::new(&share_token, &asset_token);
        let ctx = test_context();

        let effect = KernelEffect::BurnShares {
            owner: [10u8; 32],
            shares: 500,
        };

        let result = interpreter.execute_effect(&effect, &ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_soroban_interpreter_transfer_shares() {
        let share_token = MockSep41Token::new();
        let asset_token = MockSep41Token::new();
        let mut interpreter = SorobanEffectInterpreter::new(&share_token, &asset_token);
        let ctx = test_context();

        let effect = KernelEffect::TransferShares {
            from: [10u8; 32],
            to: [20u8; 32],
            shares: 250,
        };

        let result = interpreter.execute_effect(&effect, &ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_soroban_interpreter_transfer_assets() {
        let share_token = MockSep41Token::new();
        let asset_token = MockSep41Token::new();
        let mut interpreter = SorobanEffectInterpreter::new(&share_token, &asset_token);
        let ctx = test_context();

        let effect = KernelEffect::TransferAssets {
            to: [30u8; 32],
            amount: 5000,
        };

        let result = interpreter.execute_effect(&effect, &ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_soroban_interpreter_emit_event() {
        let share_token = MockSep41Token::new();
        let asset_token = MockSep41Token::new();
        let mut interpreter = SorobanEffectInterpreter::new(&share_token, &asset_token);
        let ctx = test_context();

        let effect = KernelEffect::EmitEvent {
            event: KernelEvent::RefreshCompleted { op_id: 42 },
        };

        let result = interpreter.execute_effect(&effect, &ctx);
        assert!(result.is_ok());
        assert_eq!(interpreter.events.len(), 1);
        assert!(matches!(
            interpreter.events[0],
            KernelEvent::RefreshCompleted { op_id: 42 }
        ));
    }

    #[test]
    fn test_soroban_interpreter_batch() {
        let share_token = MockSep41Token::new();
        let asset_token = MockSep41Token::new();
        let mut interpreter = SorobanEffectInterpreter::new(&share_token, &asset_token);
        let ctx = test_context();

        let effects = vec![
            KernelEffect::MintShares {
                owner: [1u8; 32],
                shares: 100,
            },
            KernelEffect::TransferAssets {
                to: [2u8; 32],
                amount: 500,
            },
            KernelEffect::EmitEvent {
                event: KernelEvent::RefreshCompleted { op_id: 1 },
            },
        ];

        let result = interpreter.execute_effects(&effects, &ctx);
        assert!(result.is_ok());

        let summary = result.unwrap();
        assert_eq!(summary.shares_minted, 100);
        assert_eq!(summary.assets_transferred, 500);
        assert_eq!(summary.events_emitted, 1);
    }

    #[test]
    fn test_soroban_interpreter_fails_on_token_error() {
        let share_token = MockSep41Token::failing();
        let asset_token = MockSep41Token::new();
        let mut interpreter = SorobanEffectInterpreter::new(&share_token, &asset_token);
        let ctx = test_context();

        let effect = KernelEffect::MintShares {
            owner: [1u8; 32],
            shares: 100,
        };

        let result = interpreter.execute_effect(&effect, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_u128_to_i128_conversion() {
        // Valid conversions
        assert!(SorobanEffectInterpreter::<MockSep41Token, MockSep41Token>::u128_to_i128(0).is_ok());
        assert!(SorobanEffectInterpreter::<MockSep41Token, MockSep41Token>::u128_to_i128(1000)
            .is_ok());
        assert!(SorobanEffectInterpreter::<MockSep41Token, MockSep41Token>::u128_to_i128(
            i128::MAX as u128
        )
        .is_ok());

        // Overflow
        assert!(SorobanEffectInterpreter::<MockSep41Token, MockSep41Token>::u128_to_i128(
            (i128::MAX as u128) + 1
        )
        .is_err());
    }
}
