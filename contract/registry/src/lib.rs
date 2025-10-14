#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    assert_one_yocto, env,
    json_types::{Base58CryptoHash, Base64VecU8},
    near, require,
    store::{IterableMap, IterableSet},
    AccountId, CryptoHash, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue, PromiseResult,
};
use near_sdk_contract_tools::{owner::Owner, Owner};
use templar_common::{
    contract::list,
    registry::{DeployMode, Deployment},
    self_ext,
};

#[derive(Debug, Clone)]
#[near(serializers = [json, borsh])]
pub enum VersionEntry {
    Code {
        hash: CryptoHash,
        code: Option<Vec<u8>>,
    },
    GlobalHash(CryptoHash),
}

impl VersionEntry {
    pub fn code_hash(&self) -> CryptoHash {
        match self {
            Self::Code { hash, .. } | Self::GlobalHash(hash) => *hash,
        }
    }
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh])]
pub enum RegistryEntry {
    Reserved,
    Deployed(Deployment),
}

#[derive(PanicOnDefault, Owner)]
#[near(contract_state)]
pub struct Contract {
    versions: IterableMap<String, VersionEntry>,
    global_contract_hashes: IterableSet<CryptoHash>,
    registry: IterableMap<AccountId, RegistryEntry>,
}

#[near]
impl Contract {
    #[init]
    pub fn new() -> Self {
        let mut self_ = Self {
            versions: IterableMap::new(b"v"),
            global_contract_hashes: IterableSet::new(b"g"),
            registry: IterableMap::new(b"r"),
        };

        self_.init(&env::predecessor_account_id());

        self_
    }

    pub fn list_versions(&self, count: Option<u32>, offset: Option<u32>) -> Vec<&String> {
        list(self.versions.keys(), offset, count)
    }

    pub fn get_version_code_hash(&self, version_key: String) -> Option<Base58CryptoHash> {
        self.versions
            .get(&version_key)
            .map(VersionEntry::code_hash)
            .map(Into::into)
    }

    pub fn list_deployments(&self, count: Option<u32>, offset: Option<u32>) -> Vec<&AccountId> {
        list(
            self.registry
                .iter()
                .filter(|(_, e)| matches!(e, RegistryEntry::Deployed(_)))
                .map(|(a, _)| a),
            offset,
            count,
        )
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
        #[serializer(borsh)] mode: DeployMode,
        #[serializer(borsh)] code: Vec<u8>,
    ) -> PromiseOrValue<()> {
        self.assert_owner();
        require!(
            !self.versions.contains_key(&version_key),
            "Version key already exists",
        );

        let hash = env::sha256_array(&code);

        match mode {
            DeployMode::Normal => {
                assert_one_yocto();
                let version_entry = VersionEntry::Code {
                    hash,
                    code: Some(code.clone()),
                };
                self.versions.insert(version_key, version_entry);
                PromiseOrValue::Value(())
            }
            DeployMode::GlobalHash => {
                let deposit = env::attached_deposit();
                require!(
                    !deposit.is_zero(),
                    "Deposit required to pay for global contract deployment",
                );
                let version_entry = VersionEntry::GlobalHash(hash);
                self.versions.insert(version_key.clone(), version_entry);
                let dummy_id: AccountId = format!("deploy.{}", env::current_account_id())
                    .parse()
                    .unwrap_or_else(|_| {
                        env::panic_str("Failed to construct deployment account ID.")
                    });
                PromiseOrValue::Promise(
                    Promise::new(dummy_id)
                        .create_account()
                        .transfer(deposit)
                        .deploy_global_contract(code)
                        .delete_account(env::current_account_id())
                        .then(self_ext!(Gas::from_tgas(6)).add_version_01_finalize(version_key)),
                )
            }
        }
    }

    #[private]
    pub fn add_version_01_finalize(&mut self, version_key: String) -> PromiseOrValue<()> {
        let result = env::promise_result(0);
        if matches!(result, PromiseResult::Successful(_)) {
            PromiseOrValue::Value(())
        } else {
            self.versions.remove(&version_key);
            PromiseOrValue::Promise(
                self_ext!(Gas::from_tgas(1)).fail("Failed to deploy global contract".to_string()),
            )
        }
    }

    #[payable]
    pub fn remove_version(&mut self, version_key: String) {
        assert_one_yocto();
        self.assert_owner();

        self.versions.entry(version_key).and_modify(|e| match e {
            VersionEntry::Code { code, .. } => {
                *code = None;
            }
            VersionEntry::GlobalHash(_) => env::panic_str("Global contract cannot be removed"),
        });
    }

    #[payable]
    pub fn deploy(
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
            .transfer(env::attached_deposit());

        match version {
            VersionEntry::Code { code, .. } => {
                let code = code
                    .as_ref()
                    .unwrap_or_else(|| env::panic_str("Version code has been deleted"));

                let minimum_deposit = env::storage_byte_cost().saturating_mul(code.len() as u128);

                require!(
                    attached_deposit >= minimum_deposit,
                    format!("Insufficient deposit to pay for storage (minimum: {minimum_deposit})"),
                );

                promise = promise.deploy_contract(code.clone());
            }
            VersionEntry::GlobalHash(hash) => promise = promise.use_global_contract(hash.to_vec()),
        }

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
                    .deploy_01_finalize(
                        market_id,
                        Deployment {
                            version_key,
                            code_hash: version.code_hash().into(),
                            block_height: env::block_height().into(),
                        },
                    ),
            )
    }

    #[private]
    pub fn deploy_01_finalize(
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
