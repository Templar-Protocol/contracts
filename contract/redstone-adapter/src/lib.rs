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
    contract::verification,
    core::process_payload,
    network::{error::Error as RedStoneError, StdEnv},
    ConfigFactory, FeedValue,
};

use crate::{
    config::{Config, DATA_STALENESS},
    event::WritePrices,
    utils::feed_to_string,
};

pub mod config;
mod event;
mod feed_data;
pub use feed_data::FeedData;
mod utils;

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

struct Payload {
    timestamp: u64,
    prices: Vec<FeedPrice>,
}

struct FeedPrice {
    feed_id: String,
    price: U256,
}

#[derive(thiserror::Error, Debug)]
pub enum FeedDataError {
    #[error("Missing feed")]
    MissingFeed,
    #[error("RedStone error: {0}")]
    RedStone(#[from] RedStoneError),
}

type Result<T> = std::result::Result<T, FeedDataError>;

impl RedStoneAdapter {
    fn feed_data<'a>(&'a self, feed_id: &str) -> Result<&'a FeedData> {
        let f = self.db.get(feed_id).ok_or(FeedDataError::MissingFeed)?;

        Ok(verification::verify_data_staleness(
            f.write_timestamp.0.into(),
            env::block_timestamp_ms().into(),
            DATA_STALENESS,
        )
        .map(|()| f)?)
    }

    fn update_feed(
        &mut self,
        is_trusted: bool,
        feed_id: &str,
        feed_data: FeedData,
    ) -> std::result::Result<FeedData, RedStoneError> {
        let now = feed_data.write_timestamp.0.into();
        let new_pkg = feed_data.package_timestamp.0.into();

        let old = self.db.get(feed_id);
        let old_write = old.map(|d| d.write_timestamp.0.into());
        let old_pkg = old.map(|d| d.package_timestamp.0.into());

        if is_trusted {
            verification::verify_trusted_update(now, old_write, old_pkg, new_pkg)?;
        } else {
            let interval = self.config.min_interval_between_updates_ms.into();
            verification::verify_untrusted_update(now, old_write, interval, old_pkg, new_pkg)?;
        }

        self.db.insert(feed_id.to_string(), feed_data.clone());

        Ok(feed_data)
    }

    fn payload(
        &self,
        feed_ids: &[String],
        payload: &[u8],
    ) -> std::result::Result<Payload, RedStoneError> {
        let feed_ids = feed_ids
            .iter()
            .map(|id| id.clone().into_bytes().into())
            .collect();
        let block_timestamp = near_sdk::env::block_timestamp_ms();

        let mut config =
            self.config
                .redstone_config::<StdEnv>((), feed_ids, block_timestamp.into())?;
        let result = process_payload(&mut config, payload.to_vec())?;

        let prices = result
            .values
            .into_iter()
            .map(|FeedValue { value, feed }| FeedPrice {
                feed_id: feed_to_string(feed),
                price: U256::from_big_endian(&value.0),
            })
            .collect();

        Ok(Payload {
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

    /// Validates prices given a payload. For pull-style usage (payload is
    /// attached to original transaction). Does not write to storage.
    ///
    /// # Errors
    ///
    /// - Feed ID is unknown or missing from payload.
    /// - [`RedStoneError`]
    #[handle_result]
    pub fn get_prices(&self, feed_ids: Vec<String>, payload: Base64VecU8) -> Result<GetPrices> {
        let Payload { timestamp, prices } = self.payload(&feed_ids, &payload.0)?;

        if prices.len() != feed_ids.len() {
            return Err(FeedDataError::MissingFeed);
        }

        Ok(GetPrices {
            timestamp: U64(timestamp),
            prices: prices
                .into_iter()
                .map(|f| U128(f.price.low_u128()))
                .collect(),
        })
    }

    /// Read prices from storage.
    ///
    /// # Errors
    ///
    /// - Feed ID missing.
    /// - [`RedStoneError`]
    #[handle_result]
    pub fn read_prices(&self, feed_ids: Vec<String>) -> Result<Vec<U256>> {
        feed_ids
            .into_iter()
            .map(|feed_id| Ok(self.feed_data(&feed_id)?.price))
            .collect()
    }

    /// Read a single feed's price data timestamp from storage.
    ///
    /// # Errors
    ///
    /// - Feed ID missing.
    /// - [`RedStoneError`]
    #[handle_result]
    pub fn read_timestamp(&self, feed_id: String) -> Result<U64> {
        Ok(self.feed_data(&feed_id)?.package_timestamp)
    }

    /// Read a single feed's price data from storage.
    ///
    /// # Errors
    ///
    /// - Feed ID missing.
    /// - [`RedStoneError`]
    #[handle_result]
    pub fn read_price_data_for_feed(&self, feed_id: String) -> Result<&FeedData> {
        self.feed_data(&feed_id)
    }

    /// Read multiple feeds' price data from storage.
    ///
    /// # Errors
    ///
    /// - Feed ID missing.
    /// - [`RedStoneError`]
    #[handle_result]
    pub fn read_price_data(&self, feed_ids: Vec<String>) -> Result<Vec<&FeedData>> {
        feed_ids
            .into_iter()
            .map(|feed_id| self.feed_data(&feed_id))
            .collect()
    }

    /// Write price data to storage.
    ///
    /// # Errors
    ///
    /// - Feed ID missing.
    /// - [`RedStoneError`]
    #[handle_result]
    pub fn write_prices(&mut self, feed_ids: Vec<String>, payload: Base64VecU8) -> Result<()> {
        let updater = env::predecessor_account_id();

        let is_trusted = Self::has_role(&updater, &Role::Updater);

        let Payload { timestamp, prices } = self.payload(&feed_ids, &payload.0)?;
        let write_timestamp = env::block_timestamp_ms();

        let updated_feeds = prices
            .into_iter()
            .flat_map(|FeedPrice { feed_id, price }| {
                let feed_data = FeedData {
                    price,
                    package_timestamp: U64(timestamp),
                    write_timestamp: U64(write_timestamp),
                };

                self.update_feed(is_trusted, &feed_id, feed_data)
                    .inspect_err(|e| near_sdk::log!("Error updating feed {feed_id}: {e}"))
            })
            .collect::<Vec<_>>();

        WritePrices {
            updater,
            updated_feeds,
        }
        .emit();

        Ok(())
    }
}
