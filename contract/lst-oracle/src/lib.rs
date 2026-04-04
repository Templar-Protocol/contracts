#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    assert_one_yocto, borsh::BorshSerialize, collections::UnorderedMap, env, near,
    serde::de::DeserializeOwned, serde_json, AccountId, BorshStorageKey, Gas, IntoStorageKey,
    PanicOnDefault, PromiseError, PromiseOrValue,
};
use near_sdk_contract_tools::{owner::Owner, Owner};
use templar_common::{
    contract::list,
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

    pub fn oracle_id(&self) -> &AccountId {
        &self.oracle_id
    }

    pub fn list_transformers(
        &self,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> Vec<PriceIdentifier> {
        list(self.transformers.keys(), offset, count)
    }

    pub fn get_transformer(&self, price_identifier: PriceIdentifier) -> Option<PriceTransformer> {
        self.transformers.get(&price_identifier)
    }

    #[payable]
    pub fn create_transformer(
        &mut self,
        price_identifier: PriceIdentifier,
        entry: PriceTransformer,
    ) {
        assert_one_yocto();
        self.assert_owner();

        if self
            .transformers
            .insert(&price_identifier, &entry)
            .is_some()
        {
            templar_common::panic_with_message("Price identifier collision");
        }
    }

    // impl Pyth:

    pub fn price_feed_exists(&self, price_identifier: PriceIdentifier) -> PromiseOrValue<bool> {
        if self.transformers.get(&price_identifier).is_some() {
            PromiseOrValue::Value(true)
        } else {
            PromiseOrValue::Promise(
                ext_pyth::ext(self.oracle_id.clone())
                    .with_static_gas(Gas::from_tgas(2))
                    .price_feed_exists(price_identifier)
                    .then(self_ext!(Gas::from_tgas(1)).price_feed_exists_01_consume_result()),
            )
        }
    }

    pub fn price_feed_exists_01_consume_result(
        &self,
        #[callback_result] result: Result<bool, PromiseError>,
    ) -> bool {
        result.unwrap_or(false)
    }

    // GAS:
    // Base: 3 (underlying oracle) + 2 (entry) + 3 (callback) + n*3 (redemption rate calls)
    // Max should be 3 + 2 + 3 + 2 * 3 = 14, plus a bit of buffer => 15

    pub fn list_ema_prices_no_older_than(
        &self,
        price_ids: Vec<PriceIdentifier>,
        age: u64,
    ) -> PromiseOrValue<OracleResponse> {
        if price_ids.is_empty() {
            return PromiseOrValue::Value(OracleResponse::new());
        }

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
            .with_static_gas(Gas::from_tgas(3))
            .list_ema_prices_no_older_than(dispatched_price_ids, age);

        for p in promises.into_iter().flatten() {
            promise = promise.and(p);
        }

        PromiseOrValue::Promise(
            promise.then(
                self_ext!(Gas::from_tgas(3))
                    .list_ema_prices_no_older_than_01_consume_results(price_ids),
            ),
        )
    }

    #[private]
    pub fn list_ema_prices_no_older_than_01_consume_results(
        &self,
        original_price_ids: Vec<PriceIdentifier>,
    ) -> OracleResponse {
        fn callback_result<T: DeserializeOwned>(index: u64) -> T {
            match env::promise_result_checked(index, 0x1000) {
                Ok(vec) => serde_json::from_slice(&vec)
                    .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string())),
                Err(e) => templar_common::panic_with_message(&format!(
                    "Promise index {index} failed: {e}"
                )),
            }
        }

        let oracle_result = callback_result::<OracleResponse>(0);
        let mut result = OracleResponse::with_capacity(oracle_result.len());

        let mut i = 1;
        for price_id in original_price_ids {
            if let Some(price) = oracle_result.get(&price_id) {
                result.insert(price_id, price.clone());
            } else {
                let Some(entry) = self.transformers.get(&price_id) else {
                    templar_common::panic_with_message(&format!(
                        "No transformer associated with price ID: {price_id}",
                    ));
                };
                let Some(price) = oracle_result.get(&entry.price_id) else {
                    templar_common::panic_with_message(&format!(
                        "Mapped price ID is not in oracle result: {price_id}",
                    ));
                };
                let input = callback_result::<Decimal>(i);
                i += 1;

                result.insert(
                    price_id,
                    price
                        .clone()
                        .and_then(|price| entry.action.apply(price, input)),
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
