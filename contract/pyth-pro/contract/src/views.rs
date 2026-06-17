//! Pyth-compatible read ABI. Method names and signatures mirror `pyth-oracle.near` so this
//! adapter is a drop-in for existing consumers (the market and the proxy-oracle's `Pyth` source).

use std::collections::HashMap;

use near_sdk::near;
use templar_common::{
    oracle::pyth::{Price, PriceIdentifier},
    Nanoseconds,
};

use crate::{Contract, ContractExt, FeedData};

impl Contract {
    /// Stored data for a consumer price id, via the feed-map seam.
    fn feed_for(&self, price_identifier: &PriceIdentifier) -> Option<&FeedData> {
        let feed_id = self.resolve(price_identifier)?;
        self.feeds.get(&feed_id)
    }

    /// Whether `feed` is no older than `age_s` seconds relative to `now`. A future-dated
    /// `publish_time_ns` (possible within the ingestion skew tolerance) is never fresh — fail
    /// closed, matching the proxy-oracle cache.
    fn is_fresh(feed: &FeedData, now: Nanoseconds, age_s: u64) -> bool {
        feed.publish_time_ns <= now && now.saturating_sub(feed.publish_time_ns).as_secs() <= age_s
    }

    /// Project one feed to a [`Price`] via `project`, applying an optional freshness bound first.
    fn read_one(
        &self,
        price_id: &PriceIdentifier,
        age: Option<u64>,
        project: impl Fn(&FeedData) -> Option<Price>,
    ) -> Option<Price> {
        let feed = self.feed_for(price_id)?;
        if let Some(age) = age {
            if !Self::is_fresh(feed, Nanoseconds::near_timestamp(), age) {
                return None;
            }
        }
        project(feed)
    }

    /// `read_one` over a list of ids (single `now` for the whole batch).
    fn read_many(
        &self,
        price_ids: Vec<PriceIdentifier>,
        age: Option<u64>,
        project: impl Fn(&FeedData) -> Option<Price>,
    ) -> HashMap<PriceIdentifier, Option<Price>> {
        let now = Nanoseconds::near_timestamp();
        price_ids
            .into_iter()
            .map(|id| {
                let price = self.feed_for(&id).and_then(|feed| {
                    if age.is_some_and(|age| !Self::is_fresh(feed, now, age)) {
                        return None;
                    }
                    project(feed)
                });
                (id, price)
            })
            .collect()
    }
}

#[near]
impl Contract {
    pub fn price_feed_exists(&self, price_identifier: PriceIdentifier) -> bool {
        self.resolve(&price_identifier)
            .is_some_and(|feed_id| self.feeds.contains_key(&feed_id))
    }

    // --- EMA prices (the variants Templar's market and proxy-oracle call) ---

    pub fn list_ema_prices_unsafe(
        &self,
        price_ids: Vec<PriceIdentifier>,
    ) -> HashMap<PriceIdentifier, Option<Price>> {
        self.read_many(price_ids, None, FeedData::to_ema_price)
    }

    pub fn list_ema_prices_no_older_than(
        &self,
        price_ids: Vec<PriceIdentifier>,
        age: u64,
    ) -> HashMap<PriceIdentifier, Option<Price>> {
        self.read_many(price_ids, Some(age), FeedData::to_ema_price)
    }

    pub fn get_ema_price_unsafe(&self, price_id: PriceIdentifier) -> Option<Price> {
        self.read_one(&price_id, None, FeedData::to_ema_price)
    }

    pub fn get_ema_price_no_older_than(
        &self,
        price_id: PriceIdentifier,
        age: u64,
    ) -> Option<Price> {
        self.read_one(&price_id, Some(age), FeedData::to_ema_price)
    }

    // --- Spot prices (fuller Pyth parity) ---

    pub fn list_prices_unsafe(
        &self,
        price_ids: Vec<PriceIdentifier>,
    ) -> HashMap<PriceIdentifier, Option<Price>> {
        self.read_many(price_ids, None, FeedData::to_spot_price)
    }

    pub fn list_prices_no_older_than(
        &self,
        price_ids: Vec<PriceIdentifier>,
        age: u64,
    ) -> HashMap<PriceIdentifier, Option<Price>> {
        self.read_many(price_ids, Some(age), FeedData::to_spot_price)
    }

    pub fn get_price_unsafe(&self, price_id: PriceIdentifier) -> Option<Price> {
        self.read_one(&price_id, None, FeedData::to_spot_price)
    }

    pub fn get_price_no_older_than(&self, price_id: PriceIdentifier, age: u64) -> Option<Price> {
        self.read_one(&price_id, Some(age), FeedData::to_spot_price)
    }

    // --- Non-suffixed variants: `pyth-oracle.near` exposes these as the `*_no_older_than` form with
    //     the contract's configured default validity window (`config.default_valid_time_period_s`). ---

    pub fn get_price(&self, price_id: PriceIdentifier) -> Option<Price> {
        self.get_price_no_older_than(price_id, self.config.default_valid_time_period_s)
    }

    pub fn get_ema_price(&self, price_id: PriceIdentifier) -> Option<Price> {
        self.get_ema_price_no_older_than(price_id, self.config.default_valid_time_period_s)
    }

    pub fn list_prices(
        &self,
        price_ids: Vec<PriceIdentifier>,
    ) -> HashMap<PriceIdentifier, Option<Price>> {
        self.list_prices_no_older_than(price_ids, self.config.default_valid_time_period_s)
    }

    pub fn list_ema_prices(
        &self,
        price_ids: Vec<PriceIdentifier>,
    ) -> HashMap<PriceIdentifier, Option<Price>> {
        self.list_ema_prices_no_older_than(price_ids, self.config.default_valid_time_period_s)
    }
}
