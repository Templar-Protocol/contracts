#![no_std]

use soroban_sdk::{contracterror, contracttype, Address, Env, Symbol, Vec};

pub const DEFAULT_TTL_THRESHOLD: u32 = 518_400;
pub const DEFAULT_TTL_EXTEND_TO: u32 = 3_110_400;
pub const MAX_MANUAL_TRIP_METADATA_LEN: usize = 1024;

pub fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
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

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum Role {
    OfflineManualTrip,
    OfflineManualUntrip,
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

#[derive(Clone)]
#[contracttype]
pub struct SourceConfig {
    pub oracle: Address,
    pub asset: Asset,
}

#[derive(Clone)]
#[contracttype]
pub struct ProxyConfig {
    pub sources: Vec<SourceConfig>,
    pub min_sources: u32,
    pub max_age_secs: Option<u64>,
    pub max_clock_drift_secs: Option<u64>,
}

#[derive(Clone)]
#[contracttype]
pub struct StepwiseChangeConfig {
    pub max_relative_change_repr: Vec<u64>,
}

#[derive(Clone)]
#[contracttype]
pub struct MonotonicRunConfig {
    pub max_streak: u32,
    pub min_relative_step_change_repr: Vec<u64>,
}

#[derive(Clone)]
#[contracttype]
pub struct WindowedChangeDeltaConfig {
    pub window_len: u32,
    pub lookback_windows: u32,
    pub max_relative_change_delta_repr: Vec<u64>,
}

#[derive(Clone)]
#[contracttype]
pub enum CircuitBreakerConfig {
    StepwiseChange(StepwiseChangeConfig),
    MonotonicRun(MonotonicRunConfig),
    WindowedChangeDelta(WindowedChangeDeltaConfig),
}

#[derive(Clone)]
#[contracttype]
pub struct SetEnforcedConfig {
    pub is_enforced: bool,
}

#[derive(Clone)]
#[contracttype]
pub struct RearmConfig {
    pub armed_after_secs: u64,
    pub accepted_history_source_code: u32,
}

#[derive(Clone)]
#[contracttype]
pub enum CircuitBreakerUpdateConfig {
    SetEnforced(SetEnforcedConfig),
    Rearm(RearmConfig),
}
