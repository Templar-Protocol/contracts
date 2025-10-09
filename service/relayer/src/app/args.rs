use std::str::FromStr;
use std::time::Duration;

use clap::{Args, Parser};
use near_crypto::SecretKey;
use near_sdk::{AccountId, NearToken};

#[derive(Parser, Debug, Clone)]
pub struct Configuration {
    /// Run the relayer on this port.
    #[arg(short, long, env = "PORT", default_value_t = 3000)]
    pub port: u16,
    /// Postgres database connection URL.
    #[arg(long, env = "DATABASE_URL")]
    pub database_url: String,
    /// NEAR RPC connection URL.
    #[arg(long, env = "RPC_URL", default_value_t = String::from("https://rpc.testnet.near.org"))]
    pub rpc_url: String,
    #[clap(flatten)]
    pub monitor: Monitor,
    #[clap(flatten)]
    pub relay: Relay,
    #[clap(flatten)]
    pub ua: UniversalAccount,
    #[clap(flatten)]
    pub cache: Cache,
    /// Broom batch size.
    #[arg(long, env = "BROOM_BATCH_SIZE", default_value_t = 16)]
    pub broom_batch_size: u32,
    /// Broom interval in seconds.
    #[arg(long, env = "BROOM_INTERVAL_SECS", default_value_t = 300)]
    pub broom_interval_secs: u64,
}

fn duration_from_secs(s: &str) -> Result<Duration, std::num::ParseIntError> {
    Ok(Duration::from_secs(u64::from_str(s)?))
}

#[derive(Args, Debug, Clone)]
pub struct Cache {
    /// Refresh the cached gas price after X seconds.
    #[arg(
        id = "cache-gase-price-secs",
        long = "cache-gase-price-secs",
        env = "CACHE_GAS_PRICE_SECS",
        value_parser = duration_from_secs,
        default_value = "600"
    )]
    pub gas_price_refresh: Duration,
    /// Refresh a cached nonce after X seconds.
    #[arg(
        id = "cache-nonce-secs",
        long = "cache-nonce-secs",
        env = "CACHE_NONCE_SECS",
        value_parser = duration_from_secs,
        default_value = "60"
    )]
    pub nonce_refresh: Duration,
    /// Refresh the cached protocol configuration after X seconds.
    #[arg(
        id = "cache-protocol-config-secs",
        long = "cache-protocol-config-secs",
        env = "CACHE_PROTOCOL_CONFIG_SECS",
        value_parser = duration_from_secs,
        default_value = "3600"
    )]
    pub protocol_config_refresh: Duration,
}

#[derive(Args, Debug, Clone)]
#[group(required = true, multiple = true)]
pub struct Monitor {
    /// Comma-separated list of registries to query for markets to monitor.
    #[arg(
        id = "monitor-registry",
        long = "monitor-registry",
        env = "MONITOR_REGISTRY",
        value_delimiter = ','
    )]
    pub registry: Vec<AccountId>,
    /// Comma-separated list of markets to monitor.
    #[arg(
        id = "monitor-market",
        long = "monitor-market",
        env = "MONITOR_MARKET",
        value_delimiter = ','
    )]
    pub market: Vec<AccountId>,
}

#[derive(Args, Debug, Clone)]
pub struct Relay {
    /// Account ID of the NEAR account that the relayer controls.
    #[arg(
        id = "relay-account-id",
        long = "relay-account-id",
        env = "RELAY_ACCOUNT_ID"
    )]
    pub account_id: AccountId,
    /// Comma-separated list of private keys to use to sign transactions for the account that the relayer controls.
    #[arg(
        id = "relay-secret-key",
        long = "relay-secret-key",
        env = "RELAY_SECRET_KEY",
        value_delimiter = ','
    )]
    pub secret_key: Vec<SecretKey>,
    /// Comma-separated list of allowed methods.
    #[arg(long, env = "ALLOWED_METHODS", default_values_t = default_allowed_methods(), value_delimiter = ',')]
    pub allowed_methods: Vec<String>,
    /// Starting allowance in yoctoNEAR.
    #[arg(long, env = "STARTING_ALLOWANCE_YOCTO", default_value_t = NearToken::from_millinear(250))]
    pub starting_allowance_yocto: NearToken,
}

#[derive(Args, Debug, Clone)]
pub struct UniversalAccount {
    /// Account ID of the NEAR account that the relayer controls for universal account creation.
    #[arg(id = "ua-account-id", long = "ua-account-id", env = "UA_ACCOUNT_ID")]
    pub account_id: AccountId,
    /// Comma-separated list of private keys to use to sign universal account creation transactions.
    #[arg(
        id = "ua-secret-key",
        long = "ua-secret-key",
        env = "UA_SECRET_KEY",
        value_delimiter = ','
    )]
    pub secret_key: Vec<SecretKey>,
    /// How difficult should the proof-of-work for universal account creation be?
    ///
    /// iterations ~ 2^difficulty
    #[arg(
        id = "ua-pow-difficulty",
        long = "ua-pow-difficulty",
        env = "UA_POW_DIFFICULTY",
        default_value_t = 17
    )]
    pub pow_difficulty: usize,
    /// How fresh must the universal account creation signature be?
    ///
    /// Based on the block hash referenced in the creation request.
    #[arg(
        id = "ua-blockref-max-age-secs",
        long = "ua-blockref-max-age-secs",
        env = "UA_BLOCKREF_MAX_AGE_SECS",
        value_parser = duration_from_secs,
        default_value = "600"
    )]
    pub blockref_max_age: Duration,
    /// Account ID of the registry from which to deploy universal accounts.
    #[arg(id = "ua-registry-id", long = "ua-registry-id", env = "UA_REGISTRY_ID")]
    pub registry_id: AccountId,
    /// Version key of the universal account contract to deploy from the registry.
    #[arg(id = "ua-version-key", long = "ua-version-key", env = "UA_VERSION_KEY")]
    pub version_key: String,
    #[arg(
        id = "ua-execute-tgas",
        long = "ua-execute-tgas",
        env = "UA_EXECUTE_TGAS",
        default_value_t = 35
    )]
    pub execute_tgas: u64,
}

fn default_allowed_methods() -> Vec<String> {
    [
        "borrow",
        "apply_interest",
        "harvest_yield",
        "withdraw_static_yield",
        "withdraw_collateral",
        "create_supply_withdrawal_request",
        "cancel_supply_withdrawal_request",
        "execute_next_supply_withdrawal_request",
        "storage_deposit",
        // Don't enable the storage withdrawal methods, because they can be used to easily extract NEAR from the relayer.
        // "storage_unregister",
        // "storage_withdraw",
    ]
    .into_iter()
    .map(|method_name| method_name.to_string())
    .collect()
}
