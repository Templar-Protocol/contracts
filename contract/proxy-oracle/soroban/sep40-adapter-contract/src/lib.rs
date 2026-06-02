#![no_std]
// Soroban contract entry points require `env: Env` and `Address` by value;
// every `#[contractimpl]` method in this crate is an ABI entry point.
#![allow(clippy::needless_pass_by_value)]

use soroban_sdk::{contract, contractevent, contractimpl, contracttype, Address, BytesN, Env, Vec};
use stellar_access::ownable::{set_owner, Ownable};
use stellar_macros::only_owner;
use templar_proxy_oracle_soroban_common::{
    extend_instance_ttl, is_zero_wasm_hash, normalized_to_sep40, Asset, ContractError, PriceData,
    PriceFeedTrait, ProxyOracleClient,
};

const MAX_HISTORY_RECORDS: u32 = 32;

soroban_sdk::contractmeta!(key = "sep", val = "40");

#[contract]
pub struct Sep40Adapter;

#[derive(Clone)]
#[contracttype]
enum DataKey {
    ParentOracle,
    PriceAsset,
    Decimals,
    Resolution,
    Base,
}

#[contractevent]
#[derive(Clone)]
pub struct Sep40AdapterDeployed {
    #[topic]
    pub owner: Address,
    #[topic]
    pub parent_oracle: Address,
    #[topic]
    pub asset: Asset,
    pub decimals: u32,
    pub resolution: u32,
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
        let storage = env.storage().instance();
        storage.set(&DataKey::ParentOracle, &parent_oracle);
        storage.set(&DataKey::PriceAsset, &asset);
        storage.set(&DataKey::Decimals, &decimals);
        storage.set(&DataKey::Resolution, &resolution);
        storage.set(&DataKey::Base, &base);
        set_owner(&env, &owner);
        Sep40AdapterDeployed {
            owner,
            parent_oracle,
            asset,
            decimals,
            resolution,
        }
        .publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn set_decimals(env: Env, decimals: u32) -> Result<(), ContractError> {
        if decimals > 18 {
            return Err(ContractError::InvalidInput);
        }
        extend_instance_ttl(&env);
        env.storage().instance().set(&DataKey::Decimals, &decimals);
        Ok(())
    }

    #[only_owner]
    pub fn set_resolution(env: Env, resolution: u32) -> Result<(), ContractError> {
        if resolution == 0 {
            return Err(ContractError::InvalidInput);
        }
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .set(&DataKey::Resolution, &resolution);
        Ok(())
    }

    #[only_owner]
    pub fn set_base(env: Env, base: Asset) {
        extend_instance_ttl(&env);
        env.storage().instance().set(&DataKey::Base, &base);
    }

    #[only_owner]
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), ContractError> {
        if is_zero_wasm_hash(&new_wasm_hash) {
            return Err(ContractError::InvalidInput);
        }
        extend_instance_ttl(&env);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    pub fn parent_oracle(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::ParentOracle)
    }

    pub fn price_asset(env: Env) -> Option<Asset> {
        env.storage().instance().get(&DataKey::PriceAsset)
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
        env.storage().instance().get(&DataKey::Base).expect("base")
    }

    fn assets(env: Env) -> Vec<Asset> {
        let asset: Asset = env
            .storage()
            .instance()
            .get(&DataKey::PriceAsset)
            .expect("asset");
        let mut v = Vec::new(&env);
        v.push_back(asset);
        v
    }

    fn decimals(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::Decimals)
            .expect("decimals")
    }

    fn resolution(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::Resolution)
            .expect("resolution")
    }

    fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData> {
        if !matches_tracked_asset(&env, &asset) {
            return None;
        }
        let parent = parent_oracle_address(&env)?;
        let decimals = stored_decimals(&env);
        let client = ProxyOracleClient::new(&env, &parent);
        let history = client.aggregated_history(&asset, &MAX_HISTORY_RECORDS)?;
        for entry in history.iter().rev() {
            if entry.timestamp == timestamp {
                return normalized_to_sep40(&entry, decimals).ok();
            }
        }
        None
    }

    fn prices(env: Env, asset: Asset, records: u32) -> Option<Vec<PriceData>> {
        if records == 0 || !matches_tracked_asset(&env, &asset) {
            return None;
        }
        let parent = parent_oracle_address(&env)?;
        let decimals = stored_decimals(&env);
        let client = ProxyOracleClient::new(&env, &parent);
        let history = client.aggregated_history(&asset, &records)?;
        if history.is_empty() {
            return None;
        }
        let mut out = Vec::new(&env);
        for entry in history.iter() {
            out.push_back(normalized_to_sep40(&entry, decimals).ok()?);
        }
        Some(out)
    }

    fn lastprice(env: Env, asset: Asset) -> Option<PriceData> {
        if !matches_tracked_asset(&env, &asset) {
            return None;
        }
        let parent = parent_oracle_address(&env)?;
        let decimals = stored_decimals(&env);
        let client = ProxyOracleClient::new(&env, &parent);
        let normalized = client.aggregated_latest(&asset)?;
        normalized_to_sep40(&normalized, decimals).ok()
    }
}

fn matches_tracked_asset(env: &Env, asset: &Asset) -> bool {
    env.storage()
        .instance()
        .get::<_, Asset>(&DataKey::PriceAsset)
        .as_ref()
        == Some(asset)
}

fn parent_oracle_address(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::ParentOracle)
}

#[allow(clippy::expect_used)]
fn stored_decimals(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::Decimals)
        .expect("decimals")
}

#[cfg(test)]
mod tests;
