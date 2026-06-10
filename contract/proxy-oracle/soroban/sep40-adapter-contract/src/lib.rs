#![no_std]
// Soroban contract entry points require `env: Env` and `Address` by value;
// every `#[contractimpl]` method in this crate is an ABI entry point.
#![allow(clippy::needless_pass_by_value)]

use soroban_sdk::{
    contract, contractevent, contractimpl, contracttype, symbol_short, Address, BytesN, Env,
    Symbol, Vec,
};
use stellar_access::ownable::{get_owner, set_owner, Ownable};
use stellar_macros::only_owner;
use templar_proxy_oracle_soroban_common::{
    extend_instance_ttl, is_zero_wasm_hash, normalized_to_sep40, Asset, ContractError, PriceData,
    PriceFeedTrait, ProxyOracleClient,
};

const MAX_HISTORY_RECORDS: u32 = 32;

/// Single instance-storage key; all adapter state lives in one `Config`
/// entry. Soroban's instance storage doesn't charge per-field, and every
/// `PriceFeedTrait` method reads at least two of these together, so the
/// per-field `DataKey` enum the adapter used to carry was pure overhead.
const CONFIG: Symbol = symbol_short!("CONFIG");

soroban_sdk::contractmeta!(key = "sep", val = "40");

/// Emitted whenever the owner-mutable triple is updated. Constant fields
/// (`parent_oracle`, `asset`) intentionally omitted: they never change,
/// and including them would just duplicate constructor data on every event.
#[contractevent]
#[derive(Clone)]
pub struct MetadataUpdated {
    pub decimals: u32,
    pub resolution: u32,
    pub base: Asset,
}

#[contract]
pub struct Sep40Adapter;

/// All adapter state. `parent_oracle` and `asset` are set once at
/// construction and never mutated thereafter — the typed setter
/// (`set_metadata`) only touches `decimals` / `resolution` / `base`.
#[contracttype]
#[derive(Clone)]
pub struct Config {
    pub parent_oracle: Address,
    pub asset: Asset,
    pub decimals: u32,
    pub resolution: u32,
    pub base: Asset,
}

#[contractimpl]
impl Sep40Adapter {
    pub fn __constructor(
        env: Env,
        owner: Address,
        parent_oracle: Address,
        asset: Asset,
        decimals: u32,
        resolution: u32,
        base: Asset,
    ) -> Result<(), ContractError> {
        if decimals > 18 || resolution == 0 {
            return Err(ContractError::InvalidInput);
        }
        extend_instance_ttl(&env);
        env.storage().instance().set(
            &CONFIG,
            &Config {
                parent_oracle,
                asset,
                decimals,
                resolution,
                base,
            },
        );
        set_owner(&env, &owner);
        Ok(())
    }

    /// Update the mutable SEP-40 metadata triple in one call. The
    /// `parent_oracle` and `asset` fields are intentionally immutable —
    /// repointing an adapter at a different parent or asset would
    /// silently invalidate downstream consumers, and the right answer is
    /// to deploy a new adapter.
    #[only_owner]
    pub fn set_metadata(
        env: Env,
        decimals: u32,
        resolution: u32,
        base: Asset,
    ) -> Result<(), ContractError> {
        if decimals > 18 || resolution == 0 {
            return Err(ContractError::InvalidInput);
        }
        extend_instance_ttl(&env);
        let mut config = load_config(&env);
        config.decimals = decimals;
        config.resolution = resolution;
        config.base = base.clone();
        env.storage().instance().set(&CONFIG, &config);
        MetadataUpdated {
            decimals,
            resolution,
            base,
        }
        .publish(&env);
        Ok(())
    }

    /// Signature matches the OpenZeppelin `Upgradeable` trait shape
    /// (`upgrade(env, new_wasm_hash, operator)`) so this adapter is
    /// forward-compatible with `stellar-contract-utils` adoption later.
    pub fn upgrade(
        env: Env,
        new_wasm_hash: BytesN<32>,
        operator: Address,
    ) -> Result<(), ContractError> {
        operator.require_auth();
        if get_owner(&env).as_ref() != Some(&operator) {
            return Err(ContractError::Unauthorized);
        }
        if is_zero_wasm_hash(&new_wasm_hash) {
            return Err(ContractError::InvalidInput);
        }
        extend_instance_ttl(&env);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    pub fn extend_ttl(env: Env) {
        extend_instance_ttl(&env);
    }

    pub fn config(env: Env) -> Option<Config> {
        extend_instance_ttl(&env);
        env.storage().instance().get(&CONFIG)
    }
}

#[contractimpl(contracttrait)]
impl Ownable for Sep40Adapter {}

// SEP-40 getters cannot return Option per the interface contract; panic on
// missing key is documented fail-closed behavior.
#[allow(clippy::expect_used)]
#[contractimpl]
impl PriceFeedTrait for Sep40Adapter {
    fn base(env: Env) -> Asset {
        load_config(&env).base
    }

    fn assets(env: Env) -> Vec<Asset> {
        let mut v = Vec::new(&env);
        v.push_back(load_config(&env).asset);
        v
    }

    fn decimals(env: Env) -> u32 {
        load_config(&env).decimals
    }

    fn resolution(env: Env) -> u32 {
        load_config(&env).resolution
    }

    fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData> {
        let config = load_config(&env);
        if config.asset != asset {
            return None;
        }
        let client = ProxyOracleClient::new(&env, &config.parent_oracle);
        let history = client.aggregated_history(&asset, &MAX_HISTORY_RECORDS)?;
        for entry in history.iter().rev() {
            if entry.timestamp == timestamp {
                return normalized_to_sep40(&entry, config.decimals).ok();
            }
        }
        None
    }

    fn prices(env: Env, asset: Asset, records: u32) -> Option<Vec<PriceData>> {
        if records == 0 {
            return None;
        }
        let config = load_config(&env);
        if config.asset != asset {
            return None;
        }
        let client = ProxyOracleClient::new(&env, &config.parent_oracle);
        let history = client.aggregated_history(&asset, &records)?;
        if history.is_empty() {
            return None;
        }
        let mut out = Vec::new(&env);
        for entry in history.iter() {
            out.push_back(normalized_to_sep40(&entry, config.decimals).ok()?);
        }
        Some(out)
    }

    fn lastprice(env: Env, asset: Asset) -> Option<PriceData> {
        let config = load_config(&env);
        if config.asset != asset {
            return None;
        }
        let client = ProxyOracleClient::new(&env, &config.parent_oracle);
        let normalized = client.aggregated_latest(&asset)?;
        normalized_to_sep40(&normalized, config.decimals).ok()
    }
}

#[allow(clippy::expect_used)]
fn load_config(env: &Env) -> Config {
    extend_instance_ttl(env);
    env.storage().instance().get(&CONFIG).expect("CONFIG")
}

#[cfg(test)]
mod tests;
