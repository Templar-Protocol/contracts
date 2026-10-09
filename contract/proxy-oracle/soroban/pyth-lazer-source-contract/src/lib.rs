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
//!
//! The freshness window applies on ingest only. `lastprice`, `price` and
//! `prices` serve the stored price whatever its age, and permissionless
//! `extend_ttl` keeps it alive, so a reader gets the publish time and decides
//! for itself. Our own path is protected by the runtime's `max_age_secs`;
//! a third-party SEP-40 consumer must apply its own bound.

use pyth_lazer_stellar_sdk::{Channel, PythLazerClient, VerifyError};
use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, symbol_short, Address,
    Bytes, BytesN, Env, Map, Symbol, Vec,
};
use stellar_access::ownable::{get_owner, set_owner, Ownable};
use stellar_macros::only_owner;
use templar_proxy_oracle_soroban_common::{
    extend_instance_ttl, is_zero_wasm_hash, normalized_to_sep40, Asset, NormalizedPrice, PriceData,
    PriceFeedTrait, DEFAULT_TTL_EXTEND_TO, DEFAULT_TTL_THRESHOLD,
};

/// Largest precision `normalized_to_sep40` can rescale without overflowing i128.
const MAX_SUPPORTED_SEP40_DECIMALS: u32 = 18;

#[cfg(any(test, feature = "testutils"))]
pub mod testutils;

pub const MICROS_PER_SEC: u64 = 1_000_000;
pub const MAX_LAZER_ENVELOPE_BYTES: u32 = 65_536;
pub const MAX_SUPPORTED_FEEDS: u32 = 64;
const MAX_INGEST_AGE_SECS: u64 = 604_800;
/// The cap matters because a future-dated price refuses updates until the ledger passes it.
const MAX_INGEST_CLOCK_DRIFT_SECS: u64 = 60;

const CONFIG: Symbol = symbol_short!("CONFIG");
const SUPPORTED_FEED_IDS: Symbol = symbol_short!("FEEDS");
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
    VerifierRejected = 6,
    VerifierInvokeFailed = 7,
}

impl From<VerifyError> for LazerSourceError {
    fn from(error: VerifyError) -> Self {
        match error {
            VerifyError::InvalidEnvelopeMagic
            | VerifyError::TruncatedEnvelope
            | VerifyError::InvalidEnvelopeLength
            | VerifyError::SignerNotTrusted
            | VerifyError::SignerExpired
            | VerifyError::InvalidRecoveryId => Self::VerifierRejected,
            VerifyError::InvalidPayloadMagic
            | VerifyError::InvalidPayloadLength
            | VerifyError::TruncatedPayload
            | VerifyError::InvalidChannel
            | VerifyError::InvalidProperty
            | VerifyError::InvalidMarketSession => Self::InvalidPayload,
            VerifyError::InvokeFailed => Self::VerifierInvokeFailed,
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

/// One event per push, not per feed: a full 64-feed registry would emit ~11.5
/// KiB of per-feed events against Soroban's ~8 KiB per-transaction cap, and
/// the test host does not model it, so the push would land only on chain.
/// Parallel vectors keep the field names out of the per-feed cost.
#[contractevent]
#[derive(Clone)]
pub struct PricesUpdated {
    #[topic]
    pub epoch: u64,
    pub feed_ids: Vec<u32>,
    pub mantissas: Vec<i64>,
    pub expos: Vec<i32>,
    pub publish_times_us: Vec<u64>,
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
        env.storage().instance().set(&VERIFICATION_EPOCH, &0_u64);
        set_owner(&env, &owner);
        Ok(())
    }

    /// Verify before the pinned SDK parser decodes the authenticated payload,
    /// then store only owner-admitted feeds with fresh, advancing timestamps.
    ///
    /// A signature or envelope Pyth's verifier refuses returns
    /// `VerifierRejected`; unparsable verified bytes return `InvalidPayload`.
    /// `VerifierInvokeFailed` means the verifier trapped or answered with a code
    /// this SDK does not know — the signal that its ABI changed under us.
    pub fn update_price_feeds(env: Env, payload: Bytes) -> Result<u32, LazerSourceError> {
        if payload.len() > MAX_LAZER_ENVELOPE_BYTES {
            return Err(LazerSourceError::InvalidPayload);
        }
        extend_instance_ttl(&env);
        let config = load_config(&env);
        let update = PythLazerClient::new(&env, &config.verifier).verify_update(&payload)?;
        if LazerChannel::from(&update.channel) != config.channel {
            return Err(LazerSourceError::ChannelMismatch);
        }
        let (oldest_allowed_us, latest_allowed_us) =
            freshness_bounds(env.ledger().timestamp(), &config.freshness)?;
        let supported = load_supported_feed_ids(&env);
        let mut prices = load_prices(&env);
        let mut updated = PricesUpdated {
            epoch: verification_epoch(&env),
            feed_ids: Vec::new(&env),
            mantissas: Vec::new(&env),
            expos: Vec::new(&env),
            publish_times_us: Vec::new(&env),
        };
        let mut stored = 0;
        for feed in &update.feeds {
            if !supported.contains(feed.feed_id) {
                continue;
            }
            let (Some(mantissa), Some(exponent), Some(publish_time_us)) =
                (feed.price, feed.exponent, feed.feed_update_timestamp)
            else {
                continue;
            };
            let stored_time_us = prices.get(feed.feed_id).map(|price| price.publish_time_us);
            if mantissa <= 0
                || publish_time_us < oldest_allowed_us
                || publish_time_us > latest_allowed_us
                || stored_time_us.is_some_and(|current| publish_time_us <= current)
            {
                continue;
            }
            let expo = i32::from(exponent);
            // A feed whose scale collapses to zero at our decimals would store
            // and report success while `lastprice` returned nothing — so
            // storing means servable.
            if normalized_to_sep40(
                &NormalizedPrice {
                    mantissa,
                    expo,
                    timestamp: publish_time_us / MICROS_PER_SEC,
                },
                config.decimals,
            )
            .is_err()
            {
                continue;
            }
            prices.set(
                feed.feed_id,
                StoredPrice {
                    mantissa,
                    expo,
                    publish_time_us,
                },
            );
            updated.feed_ids.push_back(feed.feed_id);
            updated.mantissas.push_back(mantissa);
            updated.expos.push_back(expo);
            updated.publish_times_us.push_back(publish_time_us);
            stored += 1;
        }
        if stored != 0 {
            store_prices(&env, &prices);
            updated.publish(&env);
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
        let mut prices = load_prices(&env);
        let mut prices_changed = false;
        // Prune by what is stored, not by the outgoing list, so no price can
        // outlive a registry it was never part of.
        for feed_id in prices.keys() {
            if !supported_feed_ids.contains(feed_id) {
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
        extend_instance_ttl(&env);
        operator.require_auth();
        if get_owner(&env).as_ref() != Some(&operator) {
            return Err(LazerSourceError::Unauthorized);
        }
        if is_zero_wasm_hash(&new_wasm_hash) {
            return Err(LazerSourceError::InvalidInput);
        }
        env.deployer()
            .update_current_contract_wasm(new_wasm_hash.clone());
        SourceUpgraded { new_wasm_hash }.publish(&env);
        Ok(())
    }

    /// Permissionless. Renews the instance and the stored prices, so a feed
    /// that stops being pushed still survives on keeper maintenance alone.
    pub fn extend_ttl(env: Env) {
        extend_instance_ttl(&env);
        let storage = env.storage().persistent();
        if storage.has(&PRICES) {
            storage.extend_ttl(&PRICES, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
        }
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

/// No ownerless mode exists; renouncing would brick a live source.
#[contractimpl(contracttrait)]
impl Ownable for PythLazerSource {
    fn renounce_ownership(env: &Env) {
        env.panic_with_error(LazerSourceError::InvalidInput);
    }
}

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
        || freshness.max_clock_drift_secs > MAX_INGEST_CLOCK_DRIFT_SECS
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
    let mut previous: Option<u32> = None;
    for feed_id in feed_ids.iter() {
        if previous.is_some_and(|last| last >= feed_id) {
            return Err(LazerSourceError::InvalidInput);
        }
        previous = Some(feed_id);
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
    env.storage().persistent().remove(&PRICES);
    env.storage().instance().set(&CONFIG, &config);
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
