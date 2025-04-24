#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    assert_one_yocto, env,
    json_types::{Base58CryptoHash, Base64VecU8, U64},
    near, require,
    store::IterableMap,
    AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue, PromiseResult,
};
use near_sdk_contract_tools::{owner::Owner, Owner};

#[derive(Debug, Clone)]
#[near(serializers = [borsh])]
pub struct VersionEntry {
    hash: [u8; 32],
    code: Option<Vec<u8>>,
}

impl VersionEntry {
    pub fn new(code: Vec<u8>) -> Self {
        let hash = env::sha256_array(&code);
        Self {
            hash,
            code: Some(code),
        }
    }
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh])]
pub enum RegistryEntry {
    Reserved,
    Deployed(Deployment),
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct Deployment {
    version_key: String,
    code_hash: Base58CryptoHash,
    block_height: U64,
}

#[derive(PanicOnDefault, Owner)]
#[near(contract_state)]
pub struct Contract {
    versions: IterableMap<String, VersionEntry>,
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

    pub fn get_version_code_hash(&self, version_key: String) -> Option<Base58CryptoHash> {
        self.versions
            .get(&version_key)
            .map(|version| version.hash.into())
    }

    pub fn list_deployments(&self, count: Option<u32>, offset: Option<u32>) -> Vec<&AccountId> {
        self.registry
            .iter()
            .filter(|(_, e)| matches!(e, RegistryEntry::Deployed(_)))
            .map(|(a, _)| a)
            .skip(offset.unwrap_or(0) as usize)
            .take(count.unwrap_or(u32::MAX) as usize)
            .collect()
    }

    pub fn get_deployment(&self, account_id: AccountId) -> Option<&Deployment> {
        self.registry.get(&account_id).and_then(|e| match e {
            RegistryEntry::Reserved => None,
            RegistryEntry::Deployed(deployment) => Some(deployment),
        })
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

        self.versions.insert(version_key, VersionEntry::new(code));
    }

    #[payable]
    pub fn remove_version(&mut self, version_key: String) {
        assert_one_yocto();
        self.assert_owner();

        self.versions
            .entry(version_key)
            .and_modify(|e| e.code = None);
    }

    #[payable]
    pub fn deploy_market(
        &mut self,
        name: String,
        version_key: String,
        init_args: Base64VecU8,
        full_access_keys: Option<Vec<near_sdk::PublicKey>>,
    ) -> Promise {
        require!(!name.is_empty(), "Name must not be empty");
        self.assert_owner();

        let Some(version) = self.versions.get(&version_key) else {
            env::panic_str("Version key does not exist");
        };

        let attached_deposit = env::attached_deposit();

        let code = version
            .code
            .as_ref()
            .unwrap_or_else(|| env::panic_str("Version code has been deleted"));

        let minimum_deposit = env::storage_byte_cost().saturating_mul(code.len() as u128 + 300);

        require!(
            attached_deposit >= minimum_deposit,
            format!("Insufficient deposit to pay for storage (minimum: {minimum_deposit})"),
        );

        let current_account_id = env::current_account_id();
        let market_id = format!("{name}.{current_account_id}");

        let market_id: AccountId = market_id
            .parse()
            .unwrap_or_else(|_| env::panic_str("New market ID is not a valid account ID"));

        require!(
            market_id.is_sub_account_of(&current_account_id),
            "Market ID cannot be created",
        );

        require!(
            !self.registry.contains_key(&market_id),
            "Market ID collision",
        );

        self.registry
            .insert(market_id.clone(), RegistryEntry::Reserved);

        near_sdk::log!("Deploying market to {market_id}");

        let mut promise = Promise::new(market_id.clone())
            .create_account()
            .transfer(env::attached_deposit())
            .deploy_contract(code.clone());

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
                    .deploy_market_01_finalize(
                        market_id,
                        Deployment {
                            version_key,
                            code_hash: version.hash.into(),
                            block_height: env::block_height().into(),
                        },
                    ),
            )
    }

    #[private]
    pub fn deploy_market_01_finalize(
        &mut self,
        market_id: AccountId,
        deployment: Deployment,
    ) -> PromiseOrValue<AccountId> {
        let successful = matches!(env::promise_result(0), PromiseResult::Successful(_));

        if successful {
            self.registry
                .insert(market_id.clone(), RegistryEntry::Deployed(deployment));

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
