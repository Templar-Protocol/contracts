//! Market adapter interfaces for Soroban runtime.
//!
//! Adapters abstract over local Soroban markets and cross-chain Templar markets.
//!
use soroban_sdk::{Address, Bytes, Env};
use templar_vault_kernel::{AssetId, TargetId};

use crate::error::RuntimeError;

/// Local Soroban market adapter trait.
///
/// This is the Soroban-native interface using `Env` and `Address` types.
/// Executors implement this trait for each supported local Soroban market.
///
pub trait SorobanMarketAdapter {
    /// Supply assets into the target market.
    ///
    fn supply(&self, env: &Env, asset: &Address, amount: i128) -> Result<(), RuntimeError>;

    /// Withdraw assets from the target market.
    ///
    fn withdraw(&self, env: &Env, asset: &Address, amount: i128) -> Result<(), RuntimeError>;

    /// Read total assets for a market (principal + interest).
    ///
    fn total_assets(&self, env: &Env, asset: &Address) -> Result<i128, RuntimeError>;
}

/// Settlement receipt for a cross-chain allocation attempt.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
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
pub trait SorobanCrossChainMarketAdapter {
    /// Submit a cross-chain allocation intent.
    ///
    fn submit_intent(&self, env: &Env, plan_bytes: Bytes) -> Result<u64, RuntimeError>;

    /// Settle a completed cross-chain attempt.
    ///
    fn settle(
        &self,
        env: &Env,
        op_id: u64,
        attempt_id: u64,
    ) -> Result<SettlementReceipt, RuntimeError>;

    /// Read total assets for a cross-chain market position.
    ///
    fn total_assets(&self, env: &Env, asset: &Address) -> Result<i128, RuntimeError>;
}

/// Opaque attempt identifier for cross-chain allocations.
pub type AttemptId = u64;

/// Reference to a market configuration entry.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash)]
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

impl From<(TargetId, AssetId)> for MarketRef {
    fn from(value: (TargetId, AssetId)) -> Self {
        Self::new(value.0, value.1)
    }
}

impl From<MarketRef> for (TargetId, AssetId) {
    fn from(value: MarketRef) -> Self {
        (value.market_id, value.asset_id)
    }
}

/// Test implementation of `SorobanMarketAdapter` for use with SDK testutils.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Default)]
pub struct TestMarketAdapter {
    /// Total assets to return.
    pub mock_total_assets: i128,
    /// Whether operations should fail.
    pub should_fail: bool,
}

impl TestMarketAdapter {
    /// Create a new test adapter with specified total assets.
    #[inline]
    #[must_use]
    pub const fn new(mock_total_assets: i128) -> Self {
        Self {
            mock_total_assets,
            should_fail: false,
        }
    }

    /// Create a failing test adapter.
    #[inline]
    #[must_use]
    pub const fn failing() -> Self {
        Self {
            mock_total_assets: 0,
            should_fail: true,
        }
    }
}

impl SorobanMarketAdapter for TestMarketAdapter {
    fn supply(&self, _env: &Env, _asset: &Address, _amount: i128) -> Result<(), RuntimeError> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test supply failed"));
        }
        Ok(())
    }

    fn withdraw(&self, _env: &Env, _asset: &Address, _amount: i128) -> Result<(), RuntimeError> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test withdraw failed"));
        }
        Ok(())
    }

    fn total_assets(&self, _env: &Env, _asset: &Address) -> Result<i128, RuntimeError> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test total_assets failed"));
        }
        Ok(self.mock_total_assets)
    }
}

/// Test implementation of `SorobanCrossChainMarketAdapter` for use with SDK testutils.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Default)]
pub struct TestCrossChainAdapter {
    /// Next attempt ID to return.
    pub next_attempt_id: u64,
    /// Settlement receipt to return.
    pub settlement_receipt: Option<SettlementReceipt>,
    /// Total assets to return.
    pub mock_total_assets: i128,
    /// Whether operations should fail.
    pub should_fail: bool,
}

impl TestCrossChainAdapter {
    /// Create a new test adapter.
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

impl SorobanCrossChainMarketAdapter for TestCrossChainAdapter {
    fn submit_intent(&self, _env: &Env, _plan_bytes: Bytes) -> Result<u64, RuntimeError> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test submit_intent failed"));
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
            return Err(RuntimeError::effect_failed("test settle failed"));
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

    fn total_assets(&self, _env: &Env, _asset: &Address) -> Result<i128, RuntimeError> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("test total_assets failed"));
        }
        Ok(self.mock_total_assets)
    }
}

#[cfg(test)]
mod tests;
