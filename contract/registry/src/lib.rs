#![allow(clippy::needless_pass_by_value)]

use std::{collections::HashMap, fmt::Write};

use near_sdk::{
    assert_one_yocto, borsh, env, near, require, serde_json, store::IterableMap, AccountId,
    NearToken, PanicOnDefault, Promise, PromiseResult,
};
use near_sdk_contract_tools::{owner::Owner, Owner};

#[derive(PanicOnDefault, Owner)]
#[near(contract_state)]
pub struct Contract {
    versions: IterableMap<String, Vec<u8>>,
    registry: IterableMap<AccountId, String>,
}

#[near]
impl Contract {
    #[init]
    pub fn new() -> Self {
        let mut self_ = Self {
            versions: IterableMap::new(b"v"),
            registry: IterableMap::new(b"r"),
        };

        self_.update_owner(Some(env::predecessor_account_id()));

        self_
    }

    pub fn list_versions(&self) -> Vec<&String> {
        self.versions.keys().collect()
    }

    pub fn get_deployments(&self) -> HashMap<&AccountId, &String> {
        self.registry.iter().collect()
    }

    #[payable]
    pub fn add_version(
        &mut self,
        #[serializer(borsh)] version_key: String,
        #[serializer(borsh)] code: Vec<u8>,
    ) {
        assert_one_yocto();
        self.assert_owner();
        require!(
            !self.versions.contains_key(&version_key),
            "Version key already exists",
        );

        self.versions.insert(version_key, code);
    }

    #[payable]
    pub fn deploy_market(&mut self, version_key: String, init_args: serde_json::Value) -> Promise {
        const HASH_LEN: usize = 3;
        self.assert_owner();

        let Some(version_code) = self.versions.get(&version_key) else {
            env::panic_str("Version key does not exist");
        };

        let attached_deposit = env::attached_deposit();

        require!(
            attached_deposit
                >= env::storage_byte_cost().saturating_mul(version_code.len() as u128 + 300),
            "Insufficient deposit to pay for storage",
        );

        let hash = &env::sha256_array(
            &borsh::to_vec(&(
                env::current_account_id(),
                self.registry.len(),
                version_key.clone(),
            ))
            .unwrap_or_else(|_| env::panic_str("Failed to serialize deployment triple")),
        )[0..HASH_LEN];

        let current_account_id = env::current_account_id();
        let mut market_id = String::with_capacity(HASH_LEN + 1 + current_account_id.len());

        for b in hash {
            write!(&mut market_id, "{b:x}").unwrap();
        }

        market_id.push('.');
        market_id.push_str(current_account_id.as_str());

        let market_id: AccountId = market_id
            .parse()
            .unwrap_or_else(|_| env::panic_str("New market ID is not a valid account ID"));

        require!(
            !self.registry.contains_key(&market_id),
            "Market id collision",
        );

        near_sdk::log!("Deploying market to {market_id}");

        Promise::new(market_id.clone())
            .create_account()
            .transfer(env::attached_deposit())
            .deploy_contract(version_code.clone())
            .function_call(
                "new".to_string(),
                serde_json::to_vec(&init_args).unwrap_or_else(|_| {
                    env::panic_str("Failed to serialize initialization arguments")
                }),
                NearToken::from_near(0),
                env::prepaid_gas()
                    .saturating_sub(env::used_gas())
                    .saturating_div(2),
            )
            .then(
                Self::ext(env::current_account_id())
                    .deploy_market_01_finalize(market_id, version_key),
            )
    }

    #[private]
    pub fn deploy_market_01_finalize(
        &mut self,
        market_id: AccountId,
        version_key: String,
    ) -> AccountId {
        require!(
            matches!(env::promise_result(0), PromiseResult::Successful(_)),
            "Market deployment failed",
        );

        self.registry.insert(market_id.clone(), version_key);

        market_id
    }
}

#[cfg(target_arch = "wasm32")]
mod custom_getrandom {
    #![allow(clippy::no_mangle_with_rust_abi)]

    use getrandom::{register_custom_getrandom, Error};
    use near_sdk::env;

    register_custom_getrandom!(custom_getrandom);

    #[allow(clippy::unnecessary_wraps)]
    pub fn custom_getrandom(buf: &mut [u8]) -> Result<(), Error> {
        buf.copy_from_slice(&env::random_seed_array());
        Ok(())
    }
}
