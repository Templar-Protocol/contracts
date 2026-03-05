#![allow(clippy::needless_pass_by_value)]

use std::collections::HashMap;

use near_sdk::{
    env,
    json_types::{Base64VecU8, U64},
    near, BorshStorageKey, PanicOnDefault,
};
use near_sdk_contract_tools::{rbac::Rbac, Rbac};
use templar_common::{
    oracle::redstone::{
        Config, FeedData, FeedDataError, FeedId, GetPrices, RedStoneAdapter,
        RedStoneContractInterface, RedStoneEvent, SerializableU256,
    },
    UnwrapReject,
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

    fn get_prices(&self, feed_ids: Vec<FeedId>, payload: Base64VecU8) -> GetPrices {
        self.adapter
            .get_prices(&feed_ids, &payload.0, env::block_timestamp_ms())
            .unwrap_or_reject()
    }

    fn read_prices(&self, feed_ids: Vec<FeedId>) -> HashMap<FeedId, SerializableU256> {
        let now = env::block_timestamp_ms();
        feed_ids
            .into_iter()
            .map(|feed_id| {
                let price = self.adapter.feed_data(&feed_id, now)?.price;
                Ok((feed_id, price))
            })
            .collect::<Result<HashMap<_, _>, FeedDataError>>()
            .unwrap_or_reject()
    }

    fn read_timestamp(&self, feed_id: FeedId) -> U64 {
        self.adapter
            .feed_data(&feed_id, env::block_timestamp_ms())
            .unwrap_or_reject()
            .package_timestamp
    }

    fn read_price_data_for_feed(&self, feed_id: FeedId) -> FeedData {
        self.adapter
            .feed_data(&feed_id, env::block_timestamp_ms())
            .unwrap_or_reject()
            .clone()
    }

    fn read_price_data(&self, feed_ids: Vec<FeedId>) -> HashMap<FeedId, FeedData> {
        let now = env::block_timestamp_ms();
        feed_ids
            .into_iter()
            .map(|feed_id| {
                let data = self.adapter.feed_data(&feed_id, now)?;
                Ok((feed_id, data.clone()))
            })
            .collect::<Result<HashMap<_, _>, FeedDataError>>()
            .unwrap_or_reject()
    }

    fn write_prices(&mut self, feed_ids: Vec<FeedId>, payload: Base64VecU8) {
        let updater = env::predecessor_account_id();

        let is_trusted = Self::has_role(&updater, &Role::Updater);

        let now = env::block_timestamp_ms();

        let payload = self
            .adapter
            .validate_payload(&feed_ids, &payload.0, now)
            .unwrap_or_reject();

        let writes = self.adapter.write_prices(is_trusted, payload, now);

        let updated_feeds = writes
            .into_iter()
            .filter_map(|(feed_id, result)| match result {
                Ok(feed_data) => Some(feed_data),
                Err(e) => {
                    near_sdk::log!("Failed to update feed {feed_id}: {e}");
                    None
                }
            })
            .collect::<Vec<_>>();

        RedStoneEvent::WritePrices {
            updater,
            updated_feeds,
        }
        .emit();
    }
}
