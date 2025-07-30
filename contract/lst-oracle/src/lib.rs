#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    assert_one_yocto, borsh::BorshSerialize, collections::UnorderedMap, env, near,
    serde::de::DeserializeOwned, serde_json, AccountId, BorshStorageKey, Gas, IntoStorageKey,
    PanicOnDefault, Promise, PromiseResult,
};
use near_sdk_contract_tools::{owner::Owner, Owner};
use templar_common::{
    define_list,
    number::Decimal,
    oracle::{
        price_transformer::PriceTransformer,
        pyth::{ext_pyth, OracleResponse, PriceIdentifier},
    },
    self_ext,
};

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Transformers,
}

#[derive(Debug, Owner, PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    pub oracle_id: AccountId,
    pub transformers: UnorderedMap<PriceIdentifier, PriceTransformer>,
}

#[near]
impl Contract {
    #[init]
    pub fn new(oracle_id: AccountId) -> Self {
        let mut self_ = Self {
            oracle_id,
            transformers: UnorderedMap::new(StorageKey::Transformers.into_storage_key()),
        };

        Owner::init(&mut self_, &env::predecessor_account_id());

        self_
    }

    pub fn get_oracle_id(&self) -> AccountId {
        self.oracle_id.clone()
    }

    define_list! {
        pub fn list_transformers(&self) -> Vec<PriceIdentifier> {
            self.transformers.keys()
        }
    }

    pub fn get_transformer(&self, price_identifier: PriceIdentifier) -> Option<PriceTransformer> {
        self.transformers.get(&price_identifier)
    }

    #[payable]
    pub fn create_transformer(&mut self, price_id: PriceIdentifier, entry: PriceTransformer) {
        assert_one_yocto();
        self.assert_owner();

        if self.transformers.insert(&price_id, &entry).is_some() {
            env::panic_str("Price identifier collision");
        }
    }

    // impl Pyth:

    pub fn price_feed_exists(&self, price_identifier: PriceIdentifier) -> bool {
        self.transformers.get(&price_identifier).is_some()
    }

    pub fn list_ema_prices_no_older_than(
        &self,
        price_ids: Vec<PriceIdentifier>,
        age: u64,
    ) -> Promise {
        let (dispatched_price_ids, promises): (Vec<_>, Vec<_>) = price_ids
            .iter()
            .copied()
            .map(|price_id| {
                if let Some(entry) = self.transformers.get(&price_id) {
                    (entry.price_id, Some(entry.call.promise()))
                } else {
                    (price_id, None)
                }
            })
            .unzip();

        let mut promise = ext_pyth::ext(self.oracle_id.clone())
            .list_ema_prices_no_older_than(dispatched_price_ids, age);

        for p in promises.into_iter().flatten() {
            promise = promise.and(p);
        }

        promise.then(
            self_ext!(Gas::from_tgas(4))
                .list_ema_prices_no_older_than_01_consume_results(price_ids),
        )
    }

    #[private]
    pub fn list_ema_prices_no_older_than_01_consume_results(
        &self,
        original_price_ids: Vec<PriceIdentifier>,
    ) -> OracleResponse {
        fn callback_result<T: DeserializeOwned>(index: u64) -> T {
            match env::promise_result(index) {
                PromiseResult::Successful(vec) => serde_json::from_slice(&vec).unwrap(),
                PromiseResult::Failed => env::panic_str(&format!("Promise index {index} failed")),
            }
        }

        let oracle_result = callback_result::<OracleResponse>(0);
        near_sdk::log!(
            "Original oracle result: {}",
            serde_json::to_string(&oracle_result).unwrap(),
        );
        let mut result = OracleResponse::with_capacity(oracle_result.len());

        let mut i = 1;
        for price_id in original_price_ids {
            if let Some(price) = oracle_result.get(&price_id) {
                near_sdk::log!("Original price passthrough: {price_id}");
                result.insert(price_id, price.clone());
            } else {
                near_sdk::log!("Transforming price: {price_id}");
                let Some(entry) = self.transformers.get(&price_id) else {
                    env::panic_str(&format!(
                        "No transformer associated with price ID: {price_id}",
                    ));
                };
                let Some(price) = oracle_result.get(&entry.price_id) else {
                    env::panic_str(&format!(
                        "Mapped price ID is not in oracle result: {price_id}",
                    ));
                };
                let input = callback_result::<Decimal>(i);
                i += 1;

                result.insert(
                    price_id,
                    price.clone().and_then(|price| {
                        near_sdk::log!("Applying transformation: {price_id}, {input}, {price:?}");
                        let transformed_price = entry.action.apply(price, input);
                        if transformed_price.is_none() {
                            near_sdk::log!("Transformation failed on price {price_id}");
                        }
                        transformed_price
                    }),
                );
            }
        }

        result
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
