#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    borsh::BorshSerialize, env, json_types::U64, near, require, store::IterableMap,
    BorshStorageKey, PanicOnDefault, Promise,
};

use templar_common::contract::list;
use templar_universal_account::{
    authentication::Key, Execute, ExecuteArgs, ExecutionParameters, KeyId,
};

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    next_key_index: u64,
    keys: IterableMap<KeyId, ExecutionParameters>,
}

#[derive(Debug, Clone, BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Keys,
}

#[near]
impl Contract {
    #[init]
    pub fn new(key: KeyId) -> Self {
        let mut self_ = Self {
            next_key_index: 0,
            keys: IterableMap::new(StorageKey::Keys),
        };

        self_.add_key(key);

        self_
    }

    pub fn get_key(&self, key: KeyId) -> Option<&ExecutionParameters> {
        self.keys.get(&key)
    }

    pub fn list_keys(&self, offset: Option<u32>, count: Option<u32>) -> Vec<&KeyId> {
        list(self.keys.keys(), offset, count)
    }

    #[private]
    pub fn add_key(&mut self, key: KeyId) {
        let index = self.next_key_index;
        self.next_key_index += 1;
        self.keys.insert(
            key,
            ExecutionParameters {
                index: U64(index),
                nonce: U64(0),
            },
        );
    }

    #[private]
    pub fn remove_key(&mut self, key: KeyId) {
        require!(
            self.keys.len() > 1,
            "Cannot remove last key using this function",
        );
        self.keys.remove(&key);
    }

    pub fn execute(&mut self, args: ExecuteArgs) -> Promise {
        let ExecuteArgs::Passkey { key, message } = args;
        let Some(key_entry) = self.keys.get_mut(&KeyId::Passkey(key.clone())) else {
            env::panic_str("Key does not exist")
        };

        let current_account_id = env::current_account_id();

        key.check(&message, &current_account_id, key_entry)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()))
            .execute()
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
