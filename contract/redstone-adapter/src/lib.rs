use std::str::FromStr;

use config::Config;
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

use crate::{config::DATA_STALENESS, event::WritePrices, utils::feed_to_string};

pub mod config;
mod event;
mod utils;

#[derive(Debug, Clone)]
#[near_sdk::near(serializers = [json, borsh])]
pub struct PriceData {
    pub price: String,
    pub package_timestamp: u64,
    pub write_timestamp: u64,
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
    pub db: IterableMap<String, PriceData>,
}

#[near(serializers = [json])]
pub struct GetPrice {
    pub timestamp: U64,
    pub prices: Vec<U128>,
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

    pub fn get_prices(&self, feed_ids: Vec<String>, payload: Base64VecU8) -> GetPrice {
        let (timestamp, prices) = self.get_prices_from_payload(&feed_ids, &payload.0).unwrap();

        if prices.len() != feed_ids.len() {
            templar_common::panic_with_message("Missing feed code");
        }

        GetPrice {
            timestamp: timestamp.into(),
            prices: Vec::from_iter(prices.into_iter().map(|(_, price)| price.low_u128().into())),
        }
    }

    fn update_feed(
        &mut self,
        verifier: &UpdateTimestampVerifier,
        feed_id: String,
        price_data: &PriceData,
    ) -> bool {
        let old_price_data = self.db.get(&feed_id);

        let timestamp_verification = verifier.verify_timestamp(
            price_data.write_timestamp.into(),
            old_price_data.as_ref().map(|pd| pd.write_timestamp.into()),
            self.config.min_interval_between_updates_ms.into(),
            old_price_data
                .as_ref()
                .map(|pd| pd.package_timestamp.into()),
            price_data.package_timestamp.into(),
        );

        eprintln!("{timestamp_verification:?}");

        if timestamp_verification.is_err() {
            return false;
        }

        self.db.insert(feed_id, price_data.clone());

        true
    }

    pub fn write_prices(&mut self, feed_ids: Vec<String>, payload: Base64VecU8) {
        let updater = env::predecessor_account_id();

        let verifier = if Self::has_role(&updater, &Role::Updater) {
            UpdateTimestampVerifier::Trusted
        } else {
            UpdateTimestampVerifier::Untrusted
        };

        let (package_timestamp, prices) =
            dbg!(self.get_prices_from_payload(&feed_ids, &payload.0).unwrap());
        let write_timestamp = near_sdk::env::block_timestamp_ms();

        let mut updated_feeds = vec![];

        for (feed_id, price) in prices.iter() {
            eprintln!("{feed_id}: {price}");
            let price_data = PriceData {
                price: price.to_string(),
                package_timestamp,
                write_timestamp,
            };

            if self.update_feed(&verifier, feed_id.clone(), &price_data) {
                updated_feeds.push(price_data);
            }
        }

        WritePrices {
            updated_feeds,
            updater,
        }
        .emit();
    }

    pub fn read_prices(&self, feed_ids: Vec<String>) -> Vec<U256> {
        let mut prices = vec![];

        for feed_id in feed_ids {
            let feed_data = self.db.get(&feed_id).expect("missing feed");
            let checked_feed_data = self.check_price_data(feed_data.clone()).unwrap();

            prices.push(primitive_types::U256::from_str(&checked_feed_data.price).unwrap());
        }

        prices
    }

    pub fn read_timestamp(&self, feed_id: String) -> u64 {
        let price_data = self.db.get(&feed_id).expect("missing feed");
        let checked_priced_data = self.check_price_data(price_data.clone()).unwrap();
        checked_priced_data.package_timestamp
    }

    pub fn read_price_data_for_feed(&self, feed_id: String) -> PriceData {
        let price_data = self.db.get(&feed_id).expect("missing feed");
        self.check_price_data(price_data.clone()).unwrap()
    }

    pub fn read_price_data(&self, feed_ids: Vec<String>) -> Vec<PriceData> {
        let mut price_data = Vec::with_capacity(feed_ids.len());

        for feed_id in feed_ids {
            let feed_data = self.db.get(&feed_id).expect("missing entry");
            let checked_feed_data = self.check_price_data(feed_data.clone()).unwrap();

            price_data.push(checked_feed_data);
        }

        price_data
    }

    fn check_price_data(&self, price_data: PriceData) -> Result<PriceData, RedStoneError> {
        verify_data_staleness(
            price_data.write_timestamp.into(),
            env::block_timestamp_ms().into(),
            DATA_STALENESS,
        )?;

        Ok(price_data)
    }

    pub fn unique_signer_threshold(&self) -> u64 {
        self.config.signer_count_threshold as u64
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
            dbg!(feed_ids),
            block_timestamp.into(),
        )?;
        let result = dbg!(process_payload(&mut config, payload.to_vec()))?;

        let mut prices = Vec::with_capacity(result.values.len());

        for FeedValue { value, feed } in result.values {
            let price = U256::from_big_endian(&value.0);
            let feed_string = feed_to_string(feed);
            prices.push((feed_string, price));
        }

        Ok((result.timestamp.as_millis(), prices))
    }
}
