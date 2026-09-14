// This mock implements the NEP token-receiver callbacks but ignores their
// params. They are required to be taken by value (and are read by the
// `#[near]`-generated wrappers), which trips `needless_pass_by_value`. The
// names are the JSON field names the wrapper deserializes, so they cannot be
// underscore-prefixed.
#![allow(clippy::needless_pass_by_value, unused_variables)]

use near_sdk::{json_types::U128, near, AccountId, PanicOnDefault, PromiseOrValue};

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    ft_calls: u64,
    mt_calls: u64,
}

#[near]
impl Contract {
    #[init]
    pub fn new() -> Self {
        Self {
            ft_calls: 0,
            mt_calls: 0,
        }
    }

    pub fn get_ft_calls(&self) -> u64 {
        self.ft_calls
    }

    pub fn get_mt_calls(&self) -> u64 {
        self.mt_calls
    }
}

#[near]
impl Contract {
    pub fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        self.ft_calls += 1;
        PromiseOrValue::Value(U128(0))
    }

    pub fn mt_on_transfer(
        &mut self,
        sender_id: AccountId,
        previous_owner_ids: Vec<AccountId>,
        token_ids: Vec<String>,
        amounts: Vec<U128>,
        msg: String,
    ) -> PromiseOrValue<Vec<U128>> {
        self.mt_calls += 1;
        PromiseOrValue::Value(vec![U128(0); amounts.len()])
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
