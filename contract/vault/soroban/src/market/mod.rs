//! Market adapter interfaces for Soroban runtime.
//!
//! Adapters abstract over local Soroban markets and cross-chain Templar markets.
//!
use soroban_sdk::{Address, Bytes, Env, IntoVal, Symbol, Val};
use templar_vault_kernel::{AssetId, TargetId};

use crate::error::RuntimeError;

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

/// Market adapter method names used for dynamic Soroban contract invocation.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SorobanMarketMethod {
    Supply,
    ProgressWithdrawal,
}

impl SorobanMarketMethod {
    #[inline]
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supply => "supply",
            Self::ProgressWithdrawal => "progress_withdrawal",
        }
    }
}

#[inline]
fn invoke_market_void(
    env: &Env,
    adapter: &Address,
    method: SorobanMarketMethod,
    asset: &Address,
    amount: i128,
) {
    let vault = env.current_contract_address();
    let name = Symbol::new(env, method.as_str());
    let args: soroban_sdk::Vec<Val> = (vault, asset.clone(), amount).into_val(env);
    env.invoke_contract::<Val>(adapter, &name, args);
}

/// Invoke adapter `supply(vault, asset, amount)`.
#[inline]
pub fn invoke_supply(env: &Env, adapter: &Address, asset: &Address, amount: i128) {
    invoke_market_void(env, adapter, SorobanMarketMethod::Supply, asset, amount);
}

/// Invoke adapter `progress_withdrawal(vault, asset, amount)` and return realized assets.
#[inline]
pub fn invoke_progress_withdrawal(
    env: &Env,
    adapter: &Address,
    asset: &Address,
    amount: i128,
) -> i128 {
    let vault = env.current_contract_address();
    let name = Symbol::new(env, SorobanMarketMethod::ProgressWithdrawal.as_str());
    let args: soroban_sdk::Vec<Val> = (vault, asset.clone(), amount).into_val(env);
    env.invoke_contract::<i128>(adapter, &name, args)
}

/// Invoke adapter `total_assets(asset)` and return the current market value.
#[inline]
pub fn invoke_total_assets(env: &Env, adapter: &Address, asset: &Address) -> i128 {
    let name = Symbol::new(env, "total_assets");
    let args: soroban_sdk::Vec<Val> = (asset.clone(),).into_val(env);
    env.invoke_contract::<i128>(adapter, &name, args)
}
