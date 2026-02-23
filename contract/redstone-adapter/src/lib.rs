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
    contract::verification::{verify_data_staleness, UpdateTimestampVerifier},
    core::process_payload,
    network::error::Error as RedStoneError,
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

fn panic(s: impl AsRef<str>) -> ! {
    #[cfg(target_family = "wasm")]
    {
        near_sdk::env::panic_str(s.as_ref())
    }
    #[cfg(not(target_family = "wasm"))]
    {
        panic!("{}", s.as_ref())
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

impl RedStoneAdapter {
    fn feed_data<'a>(&'a self, feed_id: &str) -> &'a FeedData {
        let f = self
            .db
            .get(feed_id)
            .unwrap_or_else(|| panic("missing feed"));

        verify_data_staleness(
            f.write_timestamp.into(),
            env::block_timestamp_ms().into(),
            DATA_STALENESS,
        )
        .unwrap_or_else(|e| panic(e.to_string()));

        f
    }

    fn update_feed(
        &mut self,
        verifier: &UpdateTimestampVerifier,
        feed_id: String,
        price_data: &FeedData,
    ) -> Result<(), RedStoneError> {
        let old_price_data = self.db.get(&feed_id);

        verifier.verify_timestamp(
            price_data.write_timestamp.into(),
            old_price_data.as_ref().map(|pd| pd.write_timestamp.into()),
            self.config.min_interval_between_updates_ms.into(),
            old_price_data
                .as_ref()
                .map(|pd| pd.package_timestamp.into()),
            price_data.package_timestamp.into(),
        )?;

        self.db.insert(feed_id, price_data.clone());

        Ok(())
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

    pub fn get_prices(&self, feed_ids: Vec<String>, payload: Base64VecU8) -> GetPrices {
        let (timestamp, prices) = self
            .get_prices_from_payload(&feed_ids, &payload.0)
            .unwrap_or_else(|e| panic(e.to_string()));

        if prices.len() != feed_ids.len() {
            panic("Missing feed code");
        }

        GetPrices {
            timestamp: timestamp.into(),
            prices: prices
                .into_iter()
                .map(|(_, price)| U128(price.low_u128()))
                .collect(),
        }
    }

    pub fn write_prices(&mut self, feed_ids: Vec<String>, payload: Base64VecU8) {
        let updater = env::predecessor_account_id();

        let verifier = if Self::has_role(&updater, &Role::Updater) {
            UpdateTimestampVerifier::Trusted
        } else {
            UpdateTimestampVerifier::Untrusted
        };

        let (package_timestamp, prices) = self
            .get_prices_from_payload(&feed_ids, &payload.0)
            .unwrap_or_else(|e| panic(e.to_string()));
        let write_timestamp = env::block_timestamp_ms();

        let mut updated_feeds = vec![];

        for (feed_id, price) in prices {
            let feed_data = FeedData {
                price,
                package_timestamp,
                write_timestamp,
            };

            if let Ok(()) = self.update_feed(&verifier, feed_id.clone(), &feed_data) {
                updated_feeds.push(feed_data);
            }
        }

        WritePrices {
            updater,
            updated_feeds,
        }
        .emit();
    }

    pub fn read_prices(&self, feed_ids: Vec<String>) -> Vec<U256> {
        feed_ids
            .into_iter()
            .map(|feed_id| self.feed_data(&feed_id).price)
            .collect()
    }

    pub fn read_timestamp(&self, feed_id: String) -> u64 {
        self.feed_data(&feed_id).package_timestamp
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

    pub fn unique_signer_threshold(&self) -> u64 {
        u64::from(self.config.signer_count_threshold)
    }

    fn get_prices_from_payload(
        &self,
        feed_ids: &[String],
        payload: &[u8],
    ) -> Result<(u64, Vec<(String, U256)>), RedStoneError> {
        let feed_ids = feed_ids
            .iter()
            .map(|id| id.clone().into_bytes().into())
            .collect();
        let block_timestamp = near_sdk::env::block_timestamp_ms();

        let mut config = self.config.redstone_config::<redstone::network::StdEnv>(
            (),
            feed_ids,
            block_timestamp.into(),
        )?;
        let result = process_payload(&mut config, payload.to_vec())?;

        let prices = result
            .values
            .into_iter()
            .map(|FeedValue { value, feed }| {
                let price = U256::from_big_endian(&value.0);
                (feed_to_string(feed), price)
            })
            .collect();

        Ok((result.timestamp.as_millis(), prices))
    }
}
