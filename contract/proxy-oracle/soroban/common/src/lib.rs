#![no_std]

use soroban_sdk::{contractclient, contracterror, contracttype, Address, BytesN, Env, Symbol, Vec};

pub const DEFAULT_TTL_THRESHOLD: u32 = 518_400;
pub const DEFAULT_TTL_EXTEND_TO: u32 = 3_110_400;
pub const MAX_MANUAL_TRIP_METADATA_LEN: usize = 1024;

pub fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
}

/// Returns true when `wasm_hash` is all zero bytes.
///
/// Implemented without `Env` so it can be used anywhere `BytesN<32>` is
/// available, including in pure validation helpers.
#[must_use]
pub fn is_zero_wasm_hash(wasm_hash: &BytesN<32>) -> bool {
    wasm_hash.to_array() == [0_u8; 32]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum Asset {
    Stellar(Address),
    Other(Symbol),
}

/// Decimals-free price representation used internally and across the
/// proxy-oracle ↔ SEP-40 adapter boundary. Adapters convert this to
/// SEP-40 `PriceData` using their own configured decimals.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct NormalizedPrice {
    pub mantissa: i64,
    pub expo: i32,
    pub timestamp: u64,
}

/// SEP-40 `PriceFeed` trait. Implemented by external price sources the
/// main proxy oracle consumes, and by the `Sep40Adapter` contracts that
/// re-expose the proxy oracle's normalized prices in SEP-40 form.
#[contractclient(name = "PriceFeedClient")]
pub trait PriceFeedTrait {
    fn base(env: Env) -> Asset;
    fn assets(env: Env) -> Vec<Asset>;
    fn decimals(env: Env) -> u32;
    fn resolution(env: Env) -> u32;
    fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData>;
    fn prices(env: Env, asset: Asset, records: u32) -> Option<Vec<PriceData>>;
    fn lastprice(env: Env, asset: Asset) -> Option<PriceData>;
}

/// Read API exposed by the proxy oracle runtime and consumed by `Sep40Adapter`
/// contracts. Returns kernel-form prices (`NormalizedPrice { mantissa, expo,
/// timestamp }`) post-aggregation; adapters re-scale to their own SEP-40
/// fixed decimals.
///
/// The proxy oracle owns the freshness check (`max_age_secs` from
/// `ProxyConfig`); `aggregated_latest` already applies it before returning.
#[contractclient(name = "ProxyOracleClient")]
pub trait ProxyOracleTrait {
    /// Latest aggregated price for `asset`, post-freshness-check. Returns
    /// `None` if no proxy is registered, the cache is empty / not accepted,
    /// or the cached entry is older than the configured `max_age_secs`.
    fn aggregated_latest(env: Env, asset: Asset) -> Option<NormalizedPrice>;
    /// Last `records` aggregated prices for `asset`, oldest first. Does not
    /// apply a freshness filter; callers that care about staleness should
    /// inspect the returned timestamps.
    fn aggregated_history(env: Env, asset: Asset, records: u32) -> Option<Vec<NormalizedPrice>>;
}

/// Convert a normalized exponent-form price to SEP-40 `PriceData` with the
/// given target decimals. Used by `Sep40Adapter` contracts.
pub fn normalized_to_sep40(
    price: &NormalizedPrice,
    decimals: u32,
) -> Result<PriceData, ContractError> {
    let decimals = i32::try_from(decimals).map_err(|_| ContractError::ConversionOverflow)?;
    let scale = decimals
        .checked_add(price.expo)
        .ok_or(ContractError::ConversionOverflow)?;
    let mut value = i128::from(price.mantissa);
    if scale >= 0 {
        value = value
            .checked_mul(
                10_i128
                    .checked_pow(scale.unsigned_abs())
                    .ok_or(ContractError::ConversionOverflow)?,
            )
            .ok_or(ContractError::ConversionOverflow)?;
    } else {
        value /= 10_i128
            .checked_pow(scale.unsigned_abs())
            .ok_or(ContractError::ConversionOverflow)?;
    }
    Ok(PriceData {
        price: value,
        timestamp: price.timestamp,
    })
}

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    AlreadyInitialized = 1,
    MissingConfig = 2,
    Unauthorized = 3,
    InvalidInput = 4,
    StorageError = 5,
    SourceUnavailable = 6,
    ResolveFailed = 7,
    ConversionOverflow = 8,
    TooManySources = 9,
    TooManyBreakers = 10,
    BreakerError = 11,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct SourceConfig {
    pub oracle: Address,
    pub asset: Asset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct ProxyConfig {
    pub sources: Vec<SourceConfig>,
    pub min_sources: u32,
    pub max_age_secs: Option<u64>,
    pub max_clock_drift_secs: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct StepwiseChangeConfig {
    pub max_relative_change_repr: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct MonotonicRunConfig {
    pub max_streak: u32,
    pub min_relative_step_change_repr: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct WindowedChangeDeltaConfig {
    pub window_len: u32,
    pub lookback_windows: u32,
    pub max_relative_change_delta_repr: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum CircuitBreakerConfig {
    StepwiseChange(StepwiseChangeConfig),
    MonotonicRun(MonotonicRunConfig),
    WindowedChangeDelta(WindowedChangeDeltaConfig),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct SetEnforcedConfig {
    pub is_enforced: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct RearmConfig {
    pub armed_after_secs: u64,
    pub accepted_history_source_code: u32,
}
