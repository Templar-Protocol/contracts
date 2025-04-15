#![allow(clippy::needless_pass_by_value)]

use std::fmt::Write;

use near_sdk::{
    assert_one_yocto, borsh, env, json_types::Base64VecU8, near, require, store::IterableMap,
    AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue, PromiseResult,
};
use near_sdk_contract_tools::{owner::Owner, Owner};

#[derive(Debug, Clone)]
#[near(serializers = [borsh])]
pub enum RegistryEntry {
    Reserved,
    Deployed { version_key: String },
}

#[derive(PanicOnDefault, Owner)]
#[near(contract_state)]
pub struct Contract {
    versions: IterableMap<String, Vec<u8>>,
    registry: IterableMap<AccountId, RegistryEntry>,
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

    pub fn list_versions(&self, count: Option<u32>, offset: Option<u32>) -> Vec<&String> {
        self.versions
            .keys()
            .skip(offset.unwrap_or(0) as usize)
            .take(count.unwrap_or(u32::MAX) as usize)
            .collect()
    }

    pub fn list_deployments(&self, count: Option<u32>, offset: Option<u32>) -> Vec<&AccountId> {
        self.registry
            .iter()
            .filter(|(_, e)| matches!(e, RegistryEntry::Deployed { .. }))
            .map(|(a, _)| a)
            .skip(offset.unwrap_or(0) as usize)
            .take(count.unwrap_or(u32::MAX) as usize)
            .collect()
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
    pub fn deploy_market(
        &mut self,
        prefix: Option<String>,
        version_key: String,
        init_args: Base64VecU8,
        full_access_keys: Option<Vec<near_sdk::PublicKey>>,
    ) -> Promise {
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
        let tail_len = HASH_LEN + 1 /* "." */ + current_account_id.len();
        let mut market_id = if let Some(mut prefix) = prefix {
            require!(!prefix.is_empty(), "Prefix must not be empty");
            prefix.push('-');
            prefix.reserve_exact(tail_len);
            prefix
        } else {
            String::with_capacity(tail_len)
        };

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

        self.registry
            .insert(market_id.clone(), RegistryEntry::Reserved);

        near_sdk::log!("Deploying market to {market_id}");

        let mut promise = Promise::new(market_id.clone())
            .create_account()
            .transfer(env::attached_deposit())
            .deploy_contract(version_code.clone());

        for key in full_access_keys.unwrap_or_default() {
            near_sdk::log!(
                "WARNING: Deploying market with full-access key {}",
                String::from(&key),
            );
            promise = promise.add_full_access_key(key);
        }

        promise
            .function_call_weight(
                "new".to_string(),
                init_args.0,
                NearToken::from_near(0),
                Gas::from_tgas(2),
                near_sdk::GasWeight(20),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_unused_gas_weight(1)
                    .with_static_gas(Gas::from_tgas(2))
                    .deploy_market_01_finalize(market_id, version_key),
            )
    }

    #[private]
    pub fn deploy_market_01_finalize(
        &mut self,
        market_id: AccountId,
        version_key: String,
    ) -> PromiseOrValue<AccountId> {
        let successful = matches!(env::promise_result(0), PromiseResult::Successful(_));

        if successful {
            self.registry
                .insert(market_id.clone(), RegistryEntry::Deployed { version_key });

            PromiseOrValue::Value(market_id)
        } else {
            self.registry.remove(&market_id);

            PromiseOrValue::Promise(
                Self::ext(env::current_account_id()).fail("Market deployment failed".to_string()),
            )
        }
    }

    #[private]
    pub fn fail(&self, message: String) {
        env::panic_str(&message);
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
