#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    env,
    json_types::{Base64VecU8, U128, U64},
    near,
    store::IterableMap,
    BorshStorageKey, PanicOnDefault,
};
use near_sdk_contract_tools::{rbac::Rbac, standard::nep297::Event, Rbac};
use primitive_types::U256;
use redstone::{
    contract::verification, core::process_payload, network::error::Error as RedStoneError,
    ConfigFactory, FeedValue,
};

use crate::{
    config::{Config, DATA_STALENESS},
    event::WritePrices,
    feed_data::FeedData,
    utils::feed_to_string,
};

pub mod config;
mod event;
pub mod feed_data;
mod utils;

pub struct NearEnv;

impl redstone::network::Environment for NearEnv {
    fn print<F: FnOnce() -> String>(print_content: F) {
        near_sdk::log!("{}", print_content());
    }
}

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
pub enum Role {
    Updater,
}

#[derive(Rbac, PanicOnDefault)]
#[rbac(roles = "Role")]
#[near(contract_state)]
pub struct RedStoneAdapter {
    pub config: Config,
    pub db: IterableMap<String, FeedData>,
}

#[near(serializers = [json])]
pub struct GetPrices {
    pub timestamp: U64,
    pub prices: Vec<U128>,
}

struct PricePackage {
    timestamp: u64,
    prices: Vec<FeedPrice>,
}

struct FeedPrice {
    feed_id: String,
    price: U256,
}

impl RedStoneAdapter {
    fn feed_data<'a>(&'a self, feed_id: &str) -> &'a FeedData {
        let f = self
            .db
            .get(feed_id)
            .unwrap_or_else(|| env::panic_str("missing feed"));

        verification::verify_data_staleness(
            f.write_timestamp.into(),
            env::block_timestamp_ms().into(),
            DATA_STALENESS,
        )
        .unwrap_or_else(|e| env::panic_str(&e.to_string()));

        f
    }

    fn update_feed(
        &mut self,
        is_trusted: bool,
        feed_id: String,
        feed_data: FeedData,
    ) -> Result<FeedData, RedStoneError> {
        let now = feed_data.write_timestamp.into();
        let new_pkg = feed_data.package_timestamp.into();

        let old = self.db.get(&feed_id);
        let old_write = old.map(|d| d.write_timestamp.into());
        let old_pkg = old.map(|d| d.package_timestamp.into());

        if is_trusted {
            verification::verify_trusted_update(now, old_write, old_pkg, new_pkg)?;
        } else {
            let interval = self.config.min_interval_between_updates_ms.into();
            verification::verify_untrusted_update(now, old_write, interval, old_pkg, new_pkg)?;
        }

        self.db.insert(feed_id, feed_data.clone());

        Ok(feed_data)
    }

    fn price_package(
        &self,
        feed_ids: &[String],
        payload: &[u8],
    ) -> Result<PricePackage, RedStoneError> {
        let feed_ids = feed_ids
            .iter()
            .map(|id| id.clone().into_bytes().into())
            .collect();
        let block_timestamp = near_sdk::env::block_timestamp_ms();

        let mut config =
            self.config
                .redstone_config::<NearEnv>((), feed_ids, block_timestamp.into())?;
        let result = process_payload(&mut config, payload.to_vec())?;

        let prices = result
            .values
            .into_iter()
            .map(|FeedValue { value, feed }| FeedPrice {
                feed_id: feed_to_string(feed),
                price: U256::from_big_endian(&value.0),
            })
            .collect();

        Ok(PricePackage {
            timestamp: result.timestamp.as_millis(),
            prices,
        })
    }
}

#[near]
impl RedStoneAdapter {
    #[init]
    pub fn new(config: Config) -> Self {
        Self {
            config,
            db: IterableMap::new(b"d"),
        }
    }

    pub fn get_config(&self) -> &Config {
        &self.config
    }

    pub fn unique_signer_threshold(&self) -> U64 {
        U64(u64::from(self.config.signer_count_threshold))
    }

    pub fn get_prices(&self, feed_ids: Vec<String>, payload: Base64VecU8) -> GetPrices {
        let PricePackage { timestamp, prices } = self
            .price_package(&feed_ids, &payload.0)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));

        if prices.len() != feed_ids.len() {
            env::panic_str("Missing feed code");
        }

        GetPrices {
            timestamp: U64(timestamp),
            prices: prices
                .into_iter()
                .map(|f| U128(f.price.low_u128()))
                .collect(),
        }
    }

    pub fn read_prices(&self, feed_ids: Vec<String>) -> Vec<U256> {
        feed_ids
            .into_iter()
            .map(|feed_id| self.feed_data(&feed_id).price)
            .collect()
    }

    pub fn read_timestamp(&self, feed_id: String) -> U64 {
        U64(self.feed_data(&feed_id).package_timestamp)
    }

    pub fn read_price_data_for_feed(&self, feed_id: String) -> &FeedData {
        self.feed_data(&feed_id)
    }

    pub fn read_price_data(&self, feed_ids: Vec<String>) -> Vec<&FeedData> {
        feed_ids
            .into_iter()
            .map(|feed_id| self.feed_data(&feed_id))
            .collect()
    }

    pub fn write_prices(&mut self, feed_ids: Vec<String>, payload: Base64VecU8) {
        let updater = env::predecessor_account_id();

        let is_trusted = Self::has_role(&updater, &Role::Updater);

        let PricePackage { timestamp, prices } = self
            .price_package(&feed_ids, &payload.0)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));
        let write_timestamp = env::block_timestamp_ms();

        let updated_feeds = prices
            .into_iter()
            .flat_map(|FeedPrice { feed_id, price }| {
                let feed_data = FeedData {
                    price,
                    package_timestamp: timestamp,
                    write_timestamp,
                };

                self.update_feed(is_trusted, feed_id, feed_data)
            })
            .collect::<Vec<_>>();

        WritePrices {
            updater,
            updated_feeds,
        }
        .emit();
    }
}
