#![no_std]
// Soroban contract entry points require `env: Env` and `Address` by value.
#![allow(clippy::needless_pass_by_value)]

//! Pyth Lazer as a SEP-40 source for the Soroban proxy oracle.
//!
//! Pyth's on-chain verifier is stateless: `verify_update` proves a payload was
//! signed by a trusted signer and hands back the bytes, with no replay
//! protection, ordering, or freshness check. This contract owns all of that —
//! a channel filter, a per-feed freshness window, a per-feed strictly advancing
//! publish time — stores one price per Lazer feed, and re-exposes it through
//! SEP-40 keyed by the feed id itself (`Asset::Other("23")` for feed 23), so
//! which feed backs which proxy asset is decided in the runtime's governed
//! source config, not here.

use pyth_lazer_stellar_sdk::{Channel, PythLazerClient};
use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, symbol_short, Address,
    Bytes, BytesN, Env, Map, Symbol, Vec,
};
use stellar_access::ownable::{set_owner, Ownable};
use stellar_macros::only_owner;
use templar_proxy_oracle_soroban_common::{
    extend_instance_ttl, normalized_to_sep40, owner_upgrade, Asset, ContractError, NormalizedPrice,
    PriceData, PriceFeedTrait, DEFAULT_TTL_EXTEND_TO, DEFAULT_TTL_THRESHOLD,
    MAX_SUPPORTED_SEP40_DECIMALS,
};

#[cfg(any(test, feature = "testutils"))]
pub mod testutils;

pub const MICROS_PER_SEC: u64 = 1_000_000;
pub const MAX_LAZER_ENVELOPE_BYTES: u32 = 65_536;
pub const MAX_SUPPORTED_FEEDS: u32 = 64;
pub const MAX_REPLAY_WATERMARKS: u32 = 256;
const MAX_INGEST_AGE_SECS: u64 = 604_800;
const MAX_CLOCK_DRIFT_SECS: u64 = 3_600;

const CONFIG: Symbol = symbol_short!("CONFIG");
const SUPPORTED_FEED_IDS: Symbol = symbol_short!("FEEDS");
const REPLAY_WATERMARKS: Symbol = symbol_short!("WATER");
const VERIFICATION_EPOCH: Symbol = symbol_short!("EPOCH");
const PRICES: Symbol = symbol_short!("PRICES");

soroban_sdk::contractmeta!(key = "sep", val = "40");

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum LazerSourceError {
    InvalidInput = 1,
    Unauthorized = 2,
    InvalidPayload = 3,
    ChannelMismatch = 4,
    ArithmeticOverflow = 5,
}

impl From<ContractError> for LazerSourceError {
    fn from(error: ContractError) -> Self {
        match error {
            ContractError::Unauthorized => Self::Unauthorized,
            _ => Self::InvalidInput,
        }
    }
}

#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LazerChannel {
    RealTime,
    FixedRate50ms,
    FixedRate200ms,
    FixedRate1000ms,
}

impl From<&Channel> for LazerChannel {
    fn from(channel: &Channel) -> Self {
        match channel {
            Channel::RealTime => Self::RealTime,
            Channel::FixedRate50ms => Self::FixedRate50ms,
            Channel::FixedRate200ms => Self::FixedRate200ms,
            Channel::FixedRate1000ms => Self::FixedRate1000ms,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreshnessConfig {
    /// Skip a feed whose update time is more than this many seconds old.
    pub max_age_secs: u64,
    /// Skip a feed whose update time is more than this many seconds ahead of the ledger.
    pub max_clock_drift_secs: u64,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub verifier: Address,
    pub base: Asset,
    pub decimals: u32,
    pub channel: LazerChannel,
    pub freshness: FreshnessConfig,
}

/// Raw stored feed: mantissa × 10^expo, microsecond publish time.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPrice {
    pub mantissa: i64,
    pub expo: i32,
    pub publish_time_us: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct PriceUpdated {
    #[topic]
    pub epoch: u64,
    pub feed_id: u32,
    pub mantissa: i64,
    pub expo: i32,
    pub publish_time_us: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct FreshnessUpdated {
    pub max_age_secs: u64,
    pub max_clock_drift_secs: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct SupportedFeedsUpdated {
    pub epoch: u64,
    pub feed_ids: Vec<u32>,
}

#[contractevent]
#[derive(Clone)]
pub struct VerificationEpochStarted {
    pub epoch: u64,
    pub verifier: Address,
    pub channel: LazerChannel,
}

#[contractevent]
#[derive(Clone)]
pub struct SourceUpgraded {
    pub new_wasm_hash: BytesN<32>,
}

/// The SEP-40 key a Lazer feed is served under: its id as a decimal symbol.
/// Hand-rolled because `format!` would link `core::fmt` into a 32 KiB wasm budget.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::expect_used)]
pub fn feed_asset(env: &Env, feed_id: u32) -> Asset {
    let mut digits = [0_u8; 10];
    let mut start = digits.len();
    let mut rest = feed_id;
    loop {
        start -= 1;
        digits[start] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    let text = core::str::from_utf8(&digits[start..]).expect("ascii digits");
    Asset::Other(Symbol::new(env, text))
}

#[contract]
pub struct PythLazerSource;

#[contractimpl]
impl PythLazerSource {
    pub fn __constructor(
        env: Env,
        owner: Address,
        config: Config,
        supported_feed_ids: Vec<u32>,
    ) -> Result<(), LazerSourceError> {
        if config.decimals > MAX_SUPPORTED_SEP40_DECIMALS {
            return Err(LazerSourceError::InvalidInput);
        }
        validate_freshness(&config.freshness)?;
        validate_supported_feed_ids(&supported_feed_ids)?;
        extend_instance_ttl(&env);
        env.storage().instance().set(&CONFIG, &config);
        env.storage()
            .instance()
            .set(&SUPPORTED_FEED_IDS, &supported_feed_ids);
        env.storage().instance().set(
            &REPLAY_WATERMARKS,
            &new_watermarks(&env, &supported_feed_ids),
        );
        env.storage().instance().set(&VERIFICATION_EPOCH, &0_u64);
        set_owner(&env, &owner);
        Ok(())
    }

    /// Verify before the pinned SDK parser decodes the authenticated payload,
    /// then store only owner-admitted feeds with fresh, advancing timestamps.
    pub fn update_price_feeds(env: Env, payload: Bytes) -> Result<u32, LazerSourceError> {
        if payload.len() > MAX_LAZER_ENVELOPE_BYTES {
            return Err(LazerSourceError::InvalidPayload);
        }
        extend_instance_ttl(&env);
        let config = load_config(&env);
        let update = PythLazerClient::new(&env, &config.verifier)
            .verify_update(&payload)
            .map_err(|_| LazerSourceError::InvalidPayload)?;
        if LazerChannel::from(&update.channel) != config.channel {
            return Err(LazerSourceError::ChannelMismatch);
        }
        let (oldest_allowed_us, latest_allowed_us) =
            freshness_bounds(env.ledger().timestamp(), &config.freshness)?;
        let mut watermarks = load_watermarks(&env);
        let mut prices = load_prices(&env);
        let epoch = verification_epoch(&env);
        let mut stored = 0;
        for feed in &update.feeds {
            let Some(watermark) = watermarks.get(feed.feed_id) else {
                continue;
            };
            let (Some(mantissa), Some(exponent), Some(publish_time_us)) =
                (feed.price, feed.exponent, feed.feed_update_timestamp)
            else {
                continue;
            };
            if mantissa <= 0
                || publish_time_us < oldest_allowed_us
                || publish_time_us > latest_allowed_us
                || watermark.is_some_and(|current| publish_time_us <= current)
            {
                continue;
            }
            let expo = i32::from(exponent);
            prices.set(
                feed.feed_id,
                StoredPrice {
                    mantissa,
                    expo,
                    publish_time_us,
                },
            );
            watermarks.set(feed.feed_id, Some(publish_time_us));
            PriceUpdated {
                epoch,
                feed_id: feed.feed_id,
                mantissa,
                expo,
                publish_time_us,
            }
            .publish(&env);
            stored += 1;
        }
        if stored != 0 {
            store_prices(&env, &prices);
            env.storage()
                .instance()
                .set(&REPLAY_WATERMARKS, &watermarks);
        }
        Ok(stored)
    }

    #[only_owner]
    pub fn set_freshness(env: Env, freshness: FreshnessConfig) -> Result<(), LazerSourceError> {
        validate_freshness(&freshness)?;
        let mut config = load_config(&env);
        if config.freshness == freshness {
            return Ok(());
        }
        extend_instance_ttl(&env);
        config.freshness = freshness.clone();
        env.storage().instance().set(&CONFIG, &config);
        FreshnessUpdated {
            max_age_secs: freshness.max_age_secs,
            max_clock_drift_secs: freshness.max_clock_drift_secs,
        }
        .publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn set_supported_feed_ids(
        env: Env,
        supported_feed_ids: Vec<u32>,
    ) -> Result<(), LazerSourceError> {
        validate_supported_feed_ids(&supported_feed_ids)?;
        let active = load_supported_feed_ids(&env);
        if active == supported_feed_ids {
            return Ok(());
        }
        let mut watermarks = load_watermarks(&env);
        for feed_id in supported_feed_ids.iter() {
            if watermarks.get(feed_id).is_none() {
                if watermarks.len() >= MAX_REPLAY_WATERMARKS {
                    return Err(LazerSourceError::InvalidInput);
                }
                watermarks.set(feed_id, None);
            }
        }
        let mut prices = load_prices(&env);
        let mut prices_changed = false;
        for feed_id in active.iter() {
            if !supported_feed_ids.contains(feed_id) && prices.contains_key(feed_id) {
                prices.remove(feed_id);
                prices_changed = true;
            }
        }
        extend_instance_ttl(&env);
        if prices_changed {
            store_prices(&env, &prices);
        }
        env.storage()
            .instance()
            .set(&SUPPORTED_FEED_IDS, &supported_feed_ids);
        env.storage()
            .instance()
            .set(&REPLAY_WATERMARKS, &watermarks);
        SupportedFeedsUpdated {
            epoch: verification_epoch(&env),
            feed_ids: supported_feed_ids,
        }
        .publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn set_verification_config(
        env: Env,
        verifier: Address,
        channel: LazerChannel,
    ) -> Result<(), LazerSourceError> {
        let mut config = load_config(&env);
        if config.verifier == verifier && config.channel == channel {
            return Ok(());
        }
        config.verifier = verifier;
        config.channel = channel;
        start_new_epoch(&env, config)
    }

    #[only_owner]
    pub fn reset_verification_epoch(env: Env) -> Result<(), LazerSourceError> {
        start_new_epoch(&env, load_config(&env))
    }

    /// Signature matches the OpenZeppelin `Upgradeable` trait shape.
    pub fn upgrade(
        env: Env,
        new_wasm_hash: BytesN<32>,
        operator: Address,
    ) -> Result<(), LazerSourceError> {
        owner_upgrade(&env, &new_wasm_hash, &operator)?;
        extend_instance_ttl(&env);
        SourceUpgraded { new_wasm_hash }.publish(&env);
        Ok(())
    }

    /// Permissionless. Stored prices renew their own TTL on every push.
    pub fn extend_ttl(env: Env) {
        extend_instance_ttl(&env);
    }

    pub fn config(env: Env) -> Option<Config> {
        extend_instance_ttl(&env);
        env.storage().instance().get(&CONFIG)
    }

    pub fn supported_feed_ids(env: Env) -> Vec<u32> {
        load_supported_feed_ids(&env)
    }

    pub fn verification_epoch(env: Env) -> u64 {
        verification_epoch(&env)
    }

    pub fn stored_price(env: Env, feed_id: u32) -> Option<StoredPrice> {
        is_supported_feed_id(&env, feed_id)
            .then(|| load_prices(&env).get(feed_id))
            .flatten()
    }
}

#[contractimpl(contracttrait)]
impl Ownable for PythLazerSource {}

/// Feeds are keyed by [`feed_asset`]. The owner-maintained registry bounds
/// storage and makes SEP-40 discovery truthful while runtime `SetProxy` remains
/// the only semantic provider-to-protocol-asset mapping.
#[contractimpl]
impl PriceFeedTrait for PythLazerSource {
    fn base(env: Env) -> Asset {
        extend_instance_ttl(&env);
        load_config(&env).base
    }

    fn assets(env: Env) -> Vec<Asset> {
        let mut assets = Vec::new(&env);
        for feed_id in load_supported_feed_ids(&env).iter() {
            assets.push_back(feed_asset(&env, feed_id));
        }
        assets
    }

    fn decimals(env: Env) -> u32 {
        extend_instance_ttl(&env);
        load_config(&env).decimals
    }

    fn resolution(_env: Env) -> u32 {
        1
    }

    fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData> {
        Self::lastprice(env, asset).filter(|price| price.timestamp == timestamp)
    }

    fn prices(env: Env, asset: Asset, records: u32) -> Option<Vec<PriceData>> {
        if records == 0 {
            return None;
        }
        let price = Self::lastprice(env.clone(), asset)?;
        Some(Vec::from_array(&env, [price]))
    }

    fn lastprice(env: Env, asset: Asset) -> Option<PriceData> {
        extend_instance_ttl(&env);
        let feed_id = supported_feed_id_for_asset(&env, &asset)?;
        let stored = load_prices(&env).get(feed_id)?;
        let normalized = NormalizedPrice {
            mantissa: stored.mantissa,
            expo: stored.expo,
            timestamp: stored.publish_time_us / MICROS_PER_SEC,
        };
        normalized_to_sep40(&normalized, load_config(&env).decimals).ok()
    }
}

fn validate_freshness(freshness: &FreshnessConfig) -> Result<(), LazerSourceError> {
    if freshness.max_age_secs == 0
        || freshness.max_age_secs > MAX_INGEST_AGE_SECS
        || freshness.max_clock_drift_secs > MAX_CLOCK_DRIFT_SECS
    {
        return Err(LazerSourceError::InvalidInput);
    }
    Ok(())
}

fn freshness_bounds(
    now_secs: u64,
    freshness: &FreshnessConfig,
) -> Result<(u64, u64), LazerSourceError> {
    let now_us = now_secs
        .checked_mul(MICROS_PER_SEC)
        .ok_or(LazerSourceError::ArithmeticOverflow)?;
    let max_age_us = freshness
        .max_age_secs
        .checked_mul(MICROS_PER_SEC)
        .ok_or(LazerSourceError::ArithmeticOverflow)?;
    let max_drift_us = freshness
        .max_clock_drift_secs
        .checked_mul(MICROS_PER_SEC)
        .ok_or(LazerSourceError::ArithmeticOverflow)?;
    Ok((
        now_us.saturating_sub(max_age_us),
        now_us.checked_add(max_drift_us).unwrap_or(u64::MAX),
    ))
}

fn validate_supported_feed_ids(feed_ids: &Vec<u32>) -> Result<(), LazerSourceError> {
    if feed_ids.is_empty() || feed_ids.len() > MAX_SUPPORTED_FEEDS {
        return Err(LazerSourceError::InvalidInput);
    }
    for index in 1..feed_ids.len() {
        let Some(previous) = feed_ids.get(index - 1) else {
            return Err(LazerSourceError::InvalidInput);
        };
        let Some(current) = feed_ids.get(index) else {
            return Err(LazerSourceError::InvalidInput);
        };
        if previous >= current {
            return Err(LazerSourceError::InvalidInput);
        }
    }
    Ok(())
}

#[allow(clippy::expect_used)]
fn load_config(env: &Env) -> Config {
    env.storage().instance().get(&CONFIG).expect("CONFIG")
}

#[allow(clippy::expect_used)]
fn load_supported_feed_ids(env: &Env) -> Vec<u32> {
    env.storage()
        .instance()
        .get(&SUPPORTED_FEED_IDS)
        .expect("SUPPORTED_FEED_IDS")
}

#[allow(clippy::expect_used)]
fn load_watermarks(env: &Env) -> Map<u32, Option<u64>> {
    env.storage()
        .instance()
        .get(&REPLAY_WATERMARKS)
        .expect("REPLAY_WATERMARKS")
}

fn load_prices(env: &Env) -> Map<u32, StoredPrice> {
    env.storage()
        .persistent()
        .get(&PRICES)
        .unwrap_or_else(|| Map::new(env))
}

fn store_prices(env: &Env, prices: &Map<u32, StoredPrice>) {
    let storage = env.storage().persistent();
    if prices.is_empty() {
        storage.remove(&PRICES);
        return;
    }
    storage.set(&PRICES, prices);
    storage.extend_ttl(&PRICES, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
}

fn new_watermarks(env: &Env, feed_ids: &Vec<u32>) -> Map<u32, Option<u64>> {
    let mut watermarks = Map::new(env);
    for feed_id in feed_ids.iter() {
        watermarks.set(feed_id, None);
    }
    watermarks
}

#[allow(clippy::expect_used)]
fn verification_epoch(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&VERIFICATION_EPOCH)
        .expect("EPOCH")
}

fn is_supported_feed_id(env: &Env, feed_id: u32) -> bool {
    load_supported_feed_ids(env).contains(feed_id)
}

fn supported_feed_id_for_asset(env: &Env, asset: &Asset) -> Option<u32> {
    load_supported_feed_ids(env)
        .iter()
        .find(|feed_id| asset == &feed_asset(env, *feed_id))
}

fn start_new_epoch(env: &Env, config: Config) -> Result<(), LazerSourceError> {
    let epoch = verification_epoch(env)
        .checked_add(1)
        .ok_or(LazerSourceError::ArithmeticOverflow)?;
    let active = load_supported_feed_ids(env);
    env.storage().persistent().remove(&PRICES);
    env.storage().instance().set(&CONFIG, &config);
    env.storage()
        .instance()
        .set(&REPLAY_WATERMARKS, &new_watermarks(env, &active));
    env.storage().instance().set(&VERIFICATION_EPOCH, &epoch);
    extend_instance_ttl(env);
    VerificationEpochStarted {
        epoch,
        verifier: config.verifier,
        channel: config.channel,
    }
    .publish(env);
    Ok(())
}

#[cfg(test)]
mod tests;
