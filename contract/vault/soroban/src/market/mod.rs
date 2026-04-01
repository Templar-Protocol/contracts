//! Market adapter interfaces for Soroban runtime.
//!
//! Adapters abstract over local Soroban markets and cross-chain Templar markets.
//!
use soroban_sdk::{Address, Env, IntoVal, Symbol, Val};

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
