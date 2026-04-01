#![allow(clippy::needless_pass_by_value)]

use std::ops::{Deref, DerefMut};

use near_sdk::{
    env,
    json_types::{U128, U64},
    near, require, PanicOnDefault, Promise,
};

use templar_common::contract::list;
use templar_universal_account::{
    contract_state::{StateVersion, VersionedState},
    impl_versioned_state, state,
    transaction::Transaction,
    ExecuteArgs, KeyId, KeyParameters, PayloadExecutionParameters,
};

type State = state::V2;

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract(pub VersionedState<State>);
impl_versioned_state!(Contract, State, state::Migration);

impl Deref for Contract {
    type Target = State;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Contract {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// # Panics
///
/// - If the list of transactions is empty.
fn transactions_to_promise(transactions: &[Transaction]) -> Promise {
    let mut promise = transactions[0].to_promise();

    for transaction in &transactions[1..] {
        promise = promise.then(transaction.to_promise());
    }

    promise
}

#[near]
impl Contract {
    #[init]
    pub fn new(key: KeyId, chain_id: U128, execute: Option<Box<[Transaction]>>) -> Self {
        let mut self_ = Self(State::new(chain_id.0));

        self_.add_key(key);

        if let Some(transactions) = execute.filter(|e| !e.is_empty()) {
            let p = transactions_to_promise(&transactions);
            p.as_return();
        }

        self_
    }

    fn payload_execution_parameters(&self, k: &KeyParameters) -> PayloadExecutionParameters {
        PayloadExecutionParameters::builder(self.chain_id)
            .with_key_parameters(*k)
            .verifying_contract(env::current_account_id())
            .build_salt()
    }

    pub fn get_key(&self, key: KeyId) -> Option<PayloadExecutionParameters> {
        let k = self.keys.get(&key)?;
        Some(self.payload_execution_parameters(k))
    }

    pub fn list_keys(&self, offset: Option<u32>, count: Option<u32>) -> Vec<&KeyId> {
        list(self.keys.keys(), offset, count)
    }

    #[private]
    pub fn add_key(&mut self, key: KeyId) {
        let index = self.next_key_index;
        self.next_key_index += 1;
        self.keys.insert(
            key.clone(),
            KeyParameters {
                block_height: U64(env::block_height()),
                index: U64(index),
                nonce: U64(0),
            },
        );
        templar_universal_account::Event::KeyAdded { key }.emit();
    }

    #[private]
    pub fn remove_key(&mut self, key: KeyId) {
        require!(
            self.keys.len() > 1,
            "Cannot remove last key using this function",
        );
        self.keys.remove(&key);
        templar_universal_account::Event::KeyRemoved { key }.emit();
    }

    pub fn execute(&mut self, args: ExecuteArgs<Box<[Transaction]>>) -> Promise {
        let key = args.key_id();
        let Some(key_entry) = self.keys.get_mut(&key) else {
            templar_common::panic_with_message("Key does not exist")
        };
        *key_entry = key_entry.next();
        let key_entry = *key_entry;
        templar_universal_account::Event::NonceExecution {
            key,
            nonce: key_entry.nonce,
        }
        .emit();

        let execution_parameters = self.payload_execution_parameters(&key_entry);
        let transactions = args
            .verify(&execution_parameters, |_| true)
            .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));

        require!(!transactions.is_empty(), "Transaction list is empty");

        transactions_to_promise(&transactions)
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
