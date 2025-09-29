#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    borsh::BorshSerialize, env, json_types::U64, near, require, serde_json, store::IterableMap,
    AccountIdRef, BorshStorageKey, PanicOnDefault, Promise,
};

use templar_common::contract::list;
use templar_universal_account::{
    authentication::{passkey, Key, SignedMessage},
    ExecutionParameters, KeyId,
};

fn execute_message<K: Key>(
    key: &K,
    msg: &K::Message,
    key_entry: &mut ExecutionParameters,
    current_account_id: &AccountIdRef,
) -> <K::Message as SignedMessage>::Output {
    require!(msg.account_id() == current_account_id, "Account mismatch");
    let p = msg.parameters();
    require!(p.index == key_entry.index, "Key index mismatch");
    require!(p.nonce.0 == key_entry.nonce.0 + 1, "Nonce mismatch");
    key_entry.nonce.0 += 1;

    key.verify_and_execute(msg)
        .unwrap_or_else(|e| env::panic_str(&e.to_string()))
}

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

    pub fn execute(&mut self, key: KeyId, message: serde_json::Value) -> Promise {
        self.execute_batch(key, vec![message])
    }

    pub fn execute_batch(&mut self, key: KeyId, messages: Vec<serde_json::Value>) -> Promise {
        let Some(key_entry) = self.keys.get_mut(&key) else {
            env::panic_str("Key does not exist")
        };

        let KeyId::Passkey(key) = key;

        let mut messages = messages.into_iter().map(|message| {
            serde_json::from_value(message).unwrap_or_else(|e| env::panic_str(&e.to_string()))
        });

        let first: passkey::Message = messages
            .next()
            .unwrap_or_else(|| env::panic_str("Empty input"));

        let current_account_id = env::current_account_id();

        let mut promise = execute_message(&key, &first, key_entry, &current_account_id);

        for message in messages {
            promise = promise.then(execute_message(
                &key,
                &message,
                key_entry,
                &current_account_id,
            ));
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
