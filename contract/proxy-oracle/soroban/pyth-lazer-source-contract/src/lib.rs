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
    Bytes, BytesN, Env, Symbol, Vec,
};
use stellar_access::ownable::{set_owner, Ownable};
use stellar_macros::only_owner;
use templar_proxy_oracle_soroban_common::{
    extend_instance_ttl, normalized_to_sep40, owner_upgrade, Asset, ContractError, NormalizedPrice,
    PriceData, PriceFeedTrait, DEFAULT_TTL_EXTEND_TO, DEFAULT_TTL_THRESHOLD, MAX_SEP40_DECIMALS,
};

#[cfg(any(test, feature = "testutils"))]
pub mod testutils;

pub const MICROS_PER_SEC: u64 = 1_000_000;

const CONFIG: Symbol = symbol_short!("CONFIG");

soroban_sdk::contractmeta!(key = "sep", val = "40");

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum LazerSourceError {
    InvalidInput = 1,
    Unauthorized = 2,
    InvalidPayload = 3,
    ChannelMismatch = 4,
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
    pub max_ahead_secs: u64,
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

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Price(Asset),
}

#[contractevent]
#[derive(Clone)]
pub struct PriceUpdated {
    #[topic]
    pub feed_id: u32,
    pub mantissa: i64,
    pub expo: i32,
    pub publish_time_us: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct FreshnessUpdated {
    pub max_age_secs: u64,
    pub max_ahead_secs: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct DecimalsUpdated {
    pub decimals: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct SourceUpgraded {
    pub new_wasm_hash: BytesN<32>,
}

/// The SEP-40 key a Lazer feed is served under: its id as a decimal symbol.
#[must_use]
pub fn feed_asset(env: &Env, feed_id: u32) -> Asset {
    let mut digits = [0_u8; 10];
    let mut start = digits.len();
    let mut rest = feed_id;
    loop {
        start -= 1;
        digits[start] = b'0' + u8::try_from(rest % 10).unwrap_or(0);
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    let text = core::str::from_utf8(&digits[start..]).unwrap_or("0");
    Asset::Other(Symbol::new(env, text))
}

#[contract]
pub struct PythLazerSource;

#[contractimpl]
impl PythLazerSource {
    pub fn __constructor(env: Env, owner: Address, config: Config) -> Result<(), LazerSourceError> {
        if config.decimals > MAX_SEP40_DECIMALS {
            return Err(LazerSourceError::InvalidInput);
        }
        validate_freshness(&config.freshness)?;
        extend_instance_ttl(&env);
        env.storage().instance().set(&CONFIG, &config);
        set_owner(&env, &owner);
        Ok(())
    }

    /// Verify a signed Lazer payload through the configured verifier and store
    /// every feed whose own update time is inside the freshness window and
    /// strictly advances. Permissionless: authenticity is cryptographic.
    /// Returns the number of feeds stored.
    pub fn update_price_feeds(env: Env, payload: Bytes) -> Result<u32, LazerSourceError> {
        extend_instance_ttl(&env);
        let config = load_config(&env);
        let update = PythLazerClient::new(&env, &config.verifier)
            .verify_update(&payload)
            .map_err(|_| LazerSourceError::InvalidPayload)?;
        if LazerChannel::from(&update.channel) != config.channel {
            return Err(LazerSourceError::ChannelMismatch);
        }
        let now = env.ledger().timestamp();
        let oldest_allowed_us = now
            .saturating_sub(config.freshness.max_age_secs)
            .saturating_mul(MICROS_PER_SEC);
        let latest_allowed_us = now
            .saturating_add(config.freshness.max_ahead_secs)
            .saturating_mul(MICROS_PER_SEC);

        let mut stored = 0;
        for feed in &update.feeds {
            let (Some(mantissa), Some(exponent), Some(publish_time_us)) =
                (feed.price, feed.exponent, feed.feed_update_timestamp)
            else {
                continue;
            };
            if mantissa <= 0
                || publish_time_us < oldest_allowed_us
                || publish_time_us > latest_allowed_us
            {
                continue;
            }
            let key = DataKey::Price(feed_asset(&env, feed.feed_id));
            let advances = env
                .storage()
                .persistent()
                .get::<_, StoredPrice>(&key)
                .is_none_or(|existing| publish_time_us > existing.publish_time_us);
            if !advances {
                continue;
            }
            let price = StoredPrice {
                mantissa,
                expo: i32::from(exponent),
                publish_time_us,
            };
            env.storage().persistent().set(&key, &price);
            env.storage().persistent().extend_ttl(
                &key,
                DEFAULT_TTL_THRESHOLD,
                DEFAULT_TTL_EXTEND_TO,
            );
            PriceUpdated {
                feed_id: feed.feed_id,
                mantissa,
                expo: price.expo,
                publish_time_us,
            }
            .publish(&env);
            stored += 1;
        }
        Ok(stored)
    }

    #[only_owner]
    pub fn set_freshness(env: Env, freshness: FreshnessConfig) -> Result<(), LazerSourceError> {
        validate_freshness(&freshness)?;
        extend_instance_ttl(&env);
        let mut config = load_config(&env);
        config.freshness = freshness.clone();
        env.storage().instance().set(&CONFIG, &config);
        FreshnessUpdated {
            max_age_secs: freshness.max_age_secs,
            max_ahead_secs: freshness.max_ahead_secs,
        }
        .publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn set_decimals(env: Env, decimals: u32) -> Result<(), LazerSourceError> {
        if decimals > MAX_SEP40_DECIMALS {
            return Err(LazerSourceError::InvalidInput);
        }
        extend_instance_ttl(&env);
        let mut config = load_config(&env);
        config.decimals = decimals;
        env.storage().instance().set(&CONFIG, &config);
        DecimalsUpdated { decimals }.publish(&env);
        Ok(())
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

    pub fn stored_price(env: Env, feed_id: u32) -> Option<StoredPrice> {
        env.storage()
            .persistent()
            .get(&DataKey::Price(feed_asset(&env, feed_id)))
    }
}

#[contractimpl(contracttrait)]
impl Ownable for PythLazerSource {}

/// Feeds are keyed by [`feed_asset`]. Only the latest price per feed is
/// retained, at second precision so the proxy oracle's freshness filter sees
/// the exact publish time: `resolution` is 1, `price` answers only for that
/// exact second, `prices` has one record. Stored feeds are not enumerated, so
/// `assets` is empty.
#[contractimpl]
impl PriceFeedTrait for PythLazerSource {
    fn base(env: Env) -> Asset {
        extend_instance_ttl(&env);
        load_config(&env).base
    }

    fn assets(env: Env) -> Vec<Asset> {
        Vec::new(&env)
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
        let stored: StoredPrice = env.storage().persistent().get(&DataKey::Price(asset))?;
        let normalized = NormalizedPrice {
            mantissa: stored.mantissa,
            expo: stored.expo,
            timestamp: stored.publish_time_us / MICROS_PER_SEC,
        };
        normalized_to_sep40(&normalized, load_config(&env).decimals).ok()
    }
}

fn validate_freshness(freshness: &FreshnessConfig) -> Result<(), LazerSourceError> {
    if freshness.max_age_secs == 0 {
        return Err(LazerSourceError::InvalidInput);
    }
    Ok(())
}

#[allow(clippy::expect_used)]
fn load_config(env: &Env) -> Config {
    env.storage().instance().get(&CONFIG).expect("CONFIG")
}

#[cfg(test)]
mod tests;
