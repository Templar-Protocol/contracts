//! Market adapter interfaces for Soroban runtime.
//!
//! Adapters abstract over local Soroban markets and cross-chain Templar markets.
//!
//! This module provides two sets of adapter interfaces:
//!
//! 1. **Soroban-style adapters** (`SorobanMarketAdapter`, `SorobanCrossChainMarketAdapter`):
//!    Use Soroban `Env` and `Address` types for direct Soroban contract integration.
//!
//! 2. **Generic adapters** (`MarketAdapter`, `CrossChainMarketAdapter`):
//!    Use kernel types (`MarketRef`) for testing and chain-agnostic logic.

use alloc::vec::Vec;
use templar_vault_kernel::{AssetId, TargetId};

use crate::error::RuntimeError;

// ---------------------------------------------------------------------------
// Soroban mock types (placeholder for soroban-sdk types)
// ---------------------------------------------------------------------------

/// Mock Soroban environment (placeholder for `soroban_sdk::Env`).
///
/// In a real Soroban contract, this would be `soroban_sdk::Env`.
/// We use a mock here so the crate can be built without soroban-sdk.
#[derive(Clone, Debug)]
pub struct Env {
    /// Ledger timestamp in nanoseconds.
    pub ledger_timestamp_ns: u64,
    /// Contract address (32 bytes).
    pub current_contract: SorobanAddress,
}

impl Env {
    /// Create a new mock environment.
    #[inline]
    #[must_use]
    pub const fn new(ledger_timestamp_ns: u64, current_contract: SorobanAddress) -> Self {
        Self {
            ledger_timestamp_ns,
            current_contract,
        }
    }

    /// Create a mock environment for testing.
    #[inline]
    #[must_use]
    pub fn mock() -> Self {
        Self {
            ledger_timestamp_ns: 1_000_000_000_000,
            current_contract: SorobanAddress([0u8; 32]),
        }
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::mock()
    }
}

/// Mock Soroban address (placeholder for `soroban_sdk::Address`).
///
/// In a real Soroban contract, this would be `soroban_sdk::Address`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SorobanAddress(pub [u8; 32]);

impl SorobanAddress {
    /// Create from raw bytes.
    #[inline]
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return the raw bytes.
    #[inline]
    #[must_use]
    pub const fn as_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl From<[u8; 32]> for SorobanAddress {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

/// Mock Soroban bytes (placeholder for `soroban_sdk::Bytes`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

impl Bytes {
    /// Create from a Vec.
    #[inline]
    #[must_use]
    pub fn from_vec(v: Vec<u8>) -> Self {
        Self(v)
    }

    /// Return as a slice.
    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

// ---------------------------------------------------------------------------
// Soroban-style market adapters (as specified in the task)
// ---------------------------------------------------------------------------

/// Local Soroban market adapter trait.
///
/// This is the Soroban-native interface using `Env` and `Address` types.
/// Executors implement this trait for each supported local Soroban market.
///
/// # Example
///
/// ```ignore
/// impl SorobanMarketAdapter for BlendMarketAdapter {
///     fn supply(&self, env: &Env, asset: SorobanAddress, amount: i128) -> Result<(), RuntimeError> {
///         // Call blend market contract
///         blend_client.supply(&env, asset, amount);
///         Ok(())
///     }
///     // ...
/// }
/// ```
pub trait SorobanMarketAdapter {
    /// Supply assets into the target market.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment.
    /// * `asset` - The asset contract address.
    /// * `amount` - The amount to supply (i128 for SEP-41 compatibility).
    fn supply(&self, env: &Env, asset: SorobanAddress, amount: i128) -> Result<(), RuntimeError>;

    /// Withdraw assets from the target market.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment.
    /// * `asset` - The asset contract address.
    /// * `amount` - The amount to withdraw (i128 for SEP-41 compatibility).
    fn withdraw(&self, env: &Env, asset: SorobanAddress, amount: i128) -> Result<(), RuntimeError>;

    /// Read total assets for a market (principal + interest).
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment.
    /// * `asset` - The asset contract address.
    ///
    /// # Returns
    ///
    /// The total assets held in this market position (i128 for SEP-41 compatibility).
    fn total_assets(&self, env: &Env, asset: SorobanAddress) -> Result<i128, RuntimeError>;
}

/// Settlement receipt for a cross-chain allocation attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettlementReceipt {
    /// Operation ID from the kernel.
    pub op_id: u64,
    /// Attempt ID returned by `submit_intent`.
    pub attempt_id: u64,
    /// New external assets value after settlement.
    pub new_external_assets: i128,
}

impl SettlementReceipt {
    /// Create a new settlement receipt.
    #[inline]
    #[must_use]
    pub const fn new(op_id: u64, attempt_id: u64, new_external_assets: i128) -> Self {
        Self {
            op_id,
            attempt_id,
            new_external_assets,
        }
    }
}

/// Cross-chain market adapter for Templar markets on other chains (via HOT/Intents).
///
/// This trait handles asynchronous cross-chain allocations through intent submission
/// and settlement verification.
///
/// # Workflow
///
/// 1. Call `submit_intent` with the allocation plan bytes.
/// 2. Wait for off-chain settlement (HOT relayer processes the intent).
/// 3. Call `settle` with the operation and attempt IDs to finalize.
///
/// # Example
///
/// ```ignore
/// impl SorobanCrossChainMarketAdapter for TemplarIntentAdapter {
///     fn submit_intent(&self, env: &Env, plan_bytes: Bytes) -> Result<u64, RuntimeError> {
///         // Store intent in outbox, return attempt ID
///         let attempt_id = self.outbox.push_intent(env, plan_bytes);
///         Ok(attempt_id)
///     }
///     // ...
/// }
/// ```
pub trait SorobanCrossChainMarketAdapter {
    /// Submit a cross-chain allocation intent.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment.
    /// * `plan_bytes` - Serialized allocation plan.
    ///
    /// # Returns
    ///
    /// An opaque attempt ID used for settlement tracking.
    fn submit_intent(&self, env: &Env, plan_bytes: Bytes) -> Result<u64, RuntimeError>;

    /// Settle a completed cross-chain attempt.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment.
    /// * `op_id` - The kernel operation ID.
    /// * `attempt_id` - The attempt ID from `submit_intent`.
    ///
    /// # Returns
    ///
    /// A settlement receipt with the new external assets value.
    fn settle(
        &self,
        env: &Env,
        op_id: u64,
        attempt_id: u64,
    ) -> Result<SettlementReceipt, RuntimeError>;

    /// Read total assets for a cross-chain market position.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment.
    /// * `asset` - The asset contract address.
    ///
    /// # Returns
    ///
    /// The total assets held in this cross-chain market position.
    fn total_assets(&self, env: &Env, asset: SorobanAddress) -> Result<i128, RuntimeError>;
}

// ---------------------------------------------------------------------------
// Generic market adapters (for testing and chain-agnostic logic)
// ---------------------------------------------------------------------------

/// Opaque attempt identifier for cross-chain allocations.
pub type AttemptId = u64;

/// Reference to a market configuration entry.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MarketRef {
    /// Market target identifier.
    pub market_id: TargetId,
    /// Asset identifier.
    pub asset_id: AssetId,
}

impl MarketRef {
    /// Create a new market reference.
    #[inline]
    #[must_use]
    pub const fn new(market_id: TargetId, asset_id: AssetId) -> Self {
        Self {
            market_id,
            asset_id,
        }
    }
}

/// Generic adapter interface for local Soroban markets.
///
/// This is a chain-agnostic interface using kernel types for testing
/// and generic reconciliation logic.
pub trait MarketAdapter {
    /// Supply assets into the target market.
    fn supply(&mut self, market: MarketRef, amount: u128) -> Result<(), RuntimeError>;
    /// Withdraw assets from the target market.
    fn withdraw(&mut self, market: MarketRef, amount: u128) -> Result<(), RuntimeError>;
    /// Read total assets for a market.
    fn total_assets(&self, market: MarketRef) -> Result<u128, RuntimeError>;
}

/// Generic adapter interface for cross-chain Templar markets via intents.
///
/// This is a chain-agnostic interface for testing and generic logic.
pub trait CrossChainMarketAdapter {
    /// Submit a cross-chain allocation intent. Returns an opaque attempt id.
    fn submit_intent(&mut self, plan_bytes: Vec<u8>) -> Result<AttemptId, RuntimeError>;
    /// Settle a completed attempt and return the new external assets.
    fn settle(
        &mut self,
        op_id: u64,
        attempt_id: AttemptId,
    ) -> Result<SettlementReceipt, RuntimeError>;
    /// Read total assets for a market.
    fn total_assets(&self, market: MarketRef) -> Result<u128, RuntimeError>;
}

// ---------------------------------------------------------------------------
// Mock implementations for testing
// ---------------------------------------------------------------------------

/// Mock implementation of `SorobanMarketAdapter` for testing.
#[derive(Clone, Debug, Default)]
pub struct MockSorobanMarketAdapter {
    /// Total assets to return.
    pub mock_total_assets: i128,
    /// Whether operations should fail.
    pub should_fail: bool,
}

impl MockSorobanMarketAdapter {
    /// Create a new mock adapter with specified total assets.
    #[inline]
    #[must_use]
    pub const fn new(mock_total_assets: i128) -> Self {
        Self {
            mock_total_assets,
            should_fail: false,
        }
    }

    /// Create a failing mock adapter.
    #[inline]
    #[must_use]
    pub const fn failing() -> Self {
        Self {
            mock_total_assets: 0,
            should_fail: true,
        }
    }
}

impl SorobanMarketAdapter for MockSorobanMarketAdapter {
    fn supply(
        &self,
        _env: &Env,
        _asset: SorobanAddress,
        _amount: i128,
    ) -> Result<(), RuntimeError> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("mock supply failed"));
        }
        Ok(())
    }

    fn withdraw(
        &self,
        _env: &Env,
        _asset: SorobanAddress,
        _amount: i128,
    ) -> Result<(), RuntimeError> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("mock withdraw failed"));
        }
        Ok(())
    }

    fn total_assets(&self, _env: &Env, _asset: SorobanAddress) -> Result<i128, RuntimeError> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("mock total_assets failed"));
        }
        Ok(self.mock_total_assets)
    }
}

/// Mock implementation of `SorobanCrossChainMarketAdapter` for testing.
#[derive(Clone, Debug, Default)]
pub struct MockSorobanCrossChainAdapter {
    /// Next attempt ID to return.
    pub next_attempt_id: u64,
    /// Settlement receipt to return.
    pub settlement_receipt: Option<SettlementReceipt>,
    /// Total assets to return.
    pub mock_total_assets: i128,
    /// Whether operations should fail.
    pub should_fail: bool,
}

impl MockSorobanCrossChainAdapter {
    /// Create a new mock adapter.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_attempt_id: 1,
            settlement_receipt: None,
            mock_total_assets: 0,
            should_fail: false,
        }
    }

    /// Set the settlement receipt to return.
    #[inline]
    pub fn with_settlement(mut self, receipt: SettlementReceipt) -> Self {
        self.settlement_receipt = Some(receipt);
        self
    }
}

impl SorobanCrossChainMarketAdapter for MockSorobanCrossChainAdapter {
    fn submit_intent(&self, _env: &Env, _plan_bytes: Bytes) -> Result<u64, RuntimeError> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("mock submit_intent failed"));
        }
        Ok(self.next_attempt_id)
    }

    fn settle(
        &self,
        _env: &Env,
        op_id: u64,
        attempt_id: u64,
    ) -> Result<SettlementReceipt, RuntimeError> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("mock settle failed"));
        }
        Ok(self
            .settlement_receipt
            .clone()
            .unwrap_or(SettlementReceipt::new(
                op_id,
                attempt_id,
                self.mock_total_assets,
            )))
    }

    fn total_assets(&self, _env: &Env, _asset: SorobanAddress) -> Result<i128, RuntimeError> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("mock total_assets failed"));
        }
        Ok(self.mock_total_assets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soroban_address_from_bytes() {
        let bytes = [1u8; 32];
        let addr = SorobanAddress::from_bytes(bytes);
        assert_eq!(addr.as_bytes(), bytes);
    }

    #[test]
    fn test_env_mock() {
        let env = Env::mock();
        assert_eq!(env.ledger_timestamp_ns, 1_000_000_000_000);
    }

    #[test]
    fn test_bytes_from_vec() {
        let v = alloc::vec![1, 2, 3];
        let bytes = Bytes::from_vec(v.clone());
        assert_eq!(bytes.as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn test_settlement_receipt_new() {
        let receipt = SettlementReceipt::new(1, 2, 1000);
        assert_eq!(receipt.op_id, 1);
        assert_eq!(receipt.attempt_id, 2);
        assert_eq!(receipt.new_external_assets, 1000);
    }

    #[test]
    fn test_market_ref_new() {
        let asset = AssetId::from([7u8; 32]);
        let market_ref = MarketRef::new(42, asset.clone());
        assert_eq!(market_ref.market_id, 42);
        assert_eq!(market_ref.asset_id, asset);
    }

    #[test]
    fn test_mock_soroban_market_adapter_success() {
        let adapter = MockSorobanMarketAdapter::new(1000);
        let env = Env::mock();
        let asset = SorobanAddress::default();

        assert!(adapter.supply(&env, asset, 100).is_ok());
        assert!(adapter.withdraw(&env, asset, 50).is_ok());
        assert_eq!(adapter.total_assets(&env, asset).unwrap(), 1000);
    }

    #[test]
    fn test_mock_soroban_market_adapter_failure() {
        let adapter = MockSorobanMarketAdapter::failing();
        let env = Env::mock();
        let asset = SorobanAddress::default();

        assert!(adapter.supply(&env, asset, 100).is_err());
        assert!(adapter.withdraw(&env, asset, 50).is_err());
        assert!(adapter.total_assets(&env, asset).is_err());
    }

    #[test]
    fn test_mock_cross_chain_adapter_submit_intent() {
        let adapter = MockSorobanCrossChainAdapter::new();
        let env = Env::mock();
        let plan = Bytes::default();

        let attempt_id = adapter.submit_intent(&env, plan).unwrap();
        assert_eq!(attempt_id, 1);
    }

    #[test]
    fn test_mock_cross_chain_adapter_settle() {
        let receipt = SettlementReceipt::new(10, 20, 5000);
        let adapter = MockSorobanCrossChainAdapter::new().with_settlement(receipt.clone());
        let env = Env::mock();

        let result = adapter.settle(&env, 10, 20).unwrap();
        assert_eq!(result, receipt);
    }

    #[test]
    fn test_mock_cross_chain_adapter_total_assets() {
        let mut adapter = MockSorobanCrossChainAdapter::new();
        adapter.mock_total_assets = 2500;
        let env = Env::mock();
        let asset = SorobanAddress::default();

        assert_eq!(adapter.total_assets(&env, asset).unwrap(), 2500);
    }
}
