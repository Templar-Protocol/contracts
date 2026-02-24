#![allow(clippy::needless_pass_by_value)]

use near_sdk::{
    env,
    json_types::{Base64VecU8, U64},
    near, BorshStorageKey, PanicOnDefault,
};
use near_sdk_contract_tools::{rbac::Rbac, standard::nep297::Event, Rbac};
use templar_common::oracle::redstone::{
    adapter::{FeedDataError, RedStoneAdapter, WritePrices},
    config::Config,
    event::RedStoneEvent,
    feed_data::{FeedData, SerializableU256},
    GetPrices, RedStoneContractInterface,
};

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
pub enum Role {
    Updater,
}

#[derive(Rbac, PanicOnDefault)]
#[rbac(roles = "Role")]
#[near(contract_state)]
pub struct Contract {
    pub adapter: RedStoneAdapter,
}

#[near]
impl Contract {
    #[init]
    pub fn new(config: Config) -> Self {
        Self {
            adapter: RedStoneAdapter::new(b"a", config),
        }
    }

    pub fn get_config(&self) -> &Config {
        &self.adapter.config
    }
}

#[near]
impl RedStoneContractInterface for Contract {
    fn unique_signer_threshold(&self) -> U64 {
        U64(u64::from(self.adapter.config.signer_count_threshold))
    }

    fn get_prices(&self, feed_ids: Vec<String>, payload: Base64VecU8) -> GetPrices {
        self.adapter
            .get_prices(&feed_ids, &payload.0, env::block_timestamp_ms())
            .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()))
    }

    fn read_prices(&self, feed_ids: Vec<String>) -> Vec<SerializableU256> {
        let now = env::block_timestamp_ms();
        feed_ids
            .into_iter()
            .map(|feed_id| Ok(self.adapter.feed_data(&feed_id, now)?.price))
            .collect::<Result<Vec<_>, FeedDataError>>()
            .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()))
    }

    fn read_timestamp(&self, feed_id: String) -> U64 {
        self.adapter
            .feed_data(&feed_id, env::block_timestamp_ms())
            .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()))
            .package_timestamp
    }

    fn read_price_data_for_feed(&self, feed_id: String) -> &FeedData {
        self.adapter
            .feed_data(&feed_id, env::block_timestamp_ms())
            .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()))
    }

    fn read_price_data(&self, feed_ids: Vec<String>) -> Vec<&FeedData> {
        let now = env::block_timestamp_ms();
        feed_ids
            .into_iter()
            .map(|feed_id| self.adapter.feed_data(&feed_id, now))
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()))
    }

    fn write_prices(&mut self, feed_ids: Vec<String>, payload: Base64VecU8) {
        let updater = env::predecessor_account_id();

        let is_trusted = Self::has_role(&updater, &Role::Updater);

        let now = env::block_timestamp_ms();

        let WritePrices {
            updated_feeds,
            failures,
        } = self
            .adapter
            .write_prices(is_trusted, &feed_ids, &payload.0, now)
            .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));

        for (feed_id, e) in failures {
            near_sdk::log!("Failed to update feed ID {feed_id}: {e}");
        }

        RedStoneEvent::WritePrices {
            updater,
            updated_feeds,
        }
        .emit();
    }
}
