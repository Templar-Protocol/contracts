use near_sdk::{
    AccountId, BorshStorageKey, IntoStorageKey, Promise, PromiseResult, assert_one_yocto,
    borsh::{self, BorshSerialize},
    collections::UnorderedMap,
    env, near,
    serde::de::DeserializeOwned,
    serde_json,
};
use near_sdk_contract_tools::{Owner, owner::Owner};
use templar_common::{
    number::Decimal,
    oracle::{
        price_transformer::PriceTransformer,
        pyth::{OracleResponse, PriceIdentifier, ext_pyth},
    },
};

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Transformers,
}

#[derive(Debug, Owner)]
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

    pub fn list_transformers(
        &self,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> Vec<PriceIdentifier> {
        self.transformers
            .keys()
            .skip(offset.map_or(0, |o| o as usize))
            .take(count.map_or(usize::MAX, |c| c as usize))
            .collect()
    }

    pub fn get_transformer(&self, price_identifier: PriceIdentifier) -> Option<PriceTransformer> {
        self.transformers.get(&price_identifier)
    }

    pub fn create_transformer(&mut self, entry: PriceTransformer) -> PriceIdentifier {
        assert_one_yocto();
        self.assert_owner();

        let id_tuple = (
            env::current_account_id(),
            entry.clone(),
            self.transformers.len(),
        );

        let price_identifier =
            PriceIdentifier(env::sha256_array(&borsh::to_vec(&id_tuple).unwrap()));

        if self
            .transformers
            .insert(&price_identifier, &entry)
            .is_some()
        {
            env::panic_str("Price identifier collision");
        }

        price_identifier
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
            .cloned()
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

        promise
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
        let mut result = OracleResponse::with_capacity(oracle_result.len());

        let mut i = 1;
        for price_id in original_price_ids {
            if let Some(price) = oracle_result.get(&price_id) {
                result.insert(price_id, price.clone());
            } else {
                let Some(entry) = self.transformers.get(&price_id) else {
                    env::panic_str(&format!(
                        "No transformer associated with price ID: {}",
                        serde_json::to_string(&price_id).unwrap(),
                    ));
                };
                let Some(price) = oracle_result.get(&entry.price_id) else {
                    env::panic_str(&format!(
                        "Mapped price ID is not in oracle result: {}",
                        serde_json::to_string(&price_id).unwrap(),
                    ));
                };
                let input = callback_result::<Decimal>(i);
                i += 1;

                result.insert(
                    price_id,
                    price.clone().and_then(|price| {
                        let transformed_price = entry.action.apply(price, input);
                        if transformed_price.is_none() {
                            near_sdk::log!(
                                "Transformation failed on price {}",
                                serde_json::to_string(&price_id).unwrap(),
                            );
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

    use getrandom::{Error, register_custom_getrandom};
    use near_sdk::env;

    register_custom_getrandom!(custom_getrandom);

    #[allow(clippy::unnecessary_wraps)]
    pub fn custom_getrandom(buf: &mut [u8]) -> Result<(), Error> {
        buf.copy_from_slice(&env::random_seed_array());
        Ok(())
    }
}
