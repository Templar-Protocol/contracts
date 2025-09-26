#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    borsh::BorshSerialize, env, json_types::U64, near, require, serde::de::DeserializeOwned,
    serde_json, store::IterableMap, BorshStorageKey, PanicOnDefault, Promise,
};

use templar_common::contract::list;

use authentication::{passkey, SignedMessage};

mod authentication;
mod key;
mod transaction;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum KeyId {
    Passkey(key::p256::PublicKey),
}

fn execute_arg<M: SignedMessage<Output = Promise>>(
    msg: &M,
    key: &M::Key,
    nonce: &mut u64,
) -> Promise {
    *nonce += 1;
    require!(msg.nonce() == *nonce, "Nonce out-of-sync");
    msg.execute(key)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()))
}

fn parse_arg<T: DeserializeOwned>(arg: serde_json::Value) -> T {
    serde_json::from_value(arg).unwrap_or_else(|e| env::panic_str(&e.to_string()))
}

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    keys: IterableMap<KeyId, U64>,
}

#[derive(Debug, Clone, BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Keys,
}

#[near]
impl Contract {
    #[init]
    pub fn new(key: KeyId, nonce: U64) -> Self {
        let mut self_ = Self {
            keys: IterableMap::new(StorageKey::Keys),
        };

        self_.add_key(key, nonce);

        self_
    }

    pub fn nonce(&self, key: KeyId) -> Option<U64> {
        self.keys.get(&key).copied()
    }

    pub fn list_keys(&self, offset: Option<u32>, count: Option<u32>) -> Vec<&KeyId> {
        list(self.keys.keys(), offset, count)
    }

    #[private]
    pub fn add_key(&mut self, key: KeyId, nonce: U64) {
        self.keys.insert(key, nonce);
    }

    #[private]
    pub fn remove_key(&mut self, key: KeyId) {
        require!(
            self.keys.len() > 1,
            "Cannot remove last key using this function",
        );
        self.keys.remove(&key);
    }

    pub fn execute(&mut self, key: KeyId, arg: serde_json::Value) -> Promise {
        self.execute_batch(key, vec![arg])
    }

    pub fn execute_batch(&mut self, key: KeyId, args: Vec<serde_json::Value>) -> Promise {
        let Some(nonce) = self.keys.get_mut(&key) else {
            env::panic_str("Key does not exist")
        };

        let KeyId::Passkey(key) = key;

        let mut args = args.into_iter().map(parse_arg);

        let first: passkey::Message = args.next().unwrap_or_else(|| env::panic_str("Empty input"));

        let mut promise = execute_arg(&first, &key, &mut nonce.0);

        for arg in args {
            promise = promise.then(execute_arg(&arg, &key, &mut nonce.0));
        }

        promise
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
