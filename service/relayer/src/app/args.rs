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
    pub pyth: Pyth,
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

fn gas_from_tgas(s: &str) -> Result<near_sdk::Gas, std::num::ParseIntError> {
    Ok(near_sdk::Gas::from_tgas(u64::from_str(s)?))
}

#[derive(Args, Debug, Clone)]
pub struct Pyth {
    /// Pyth Hermes API URL. See: <https://docs.pyth.network/price-feeds/core/api-reference>
    #[arg(
        long = "pyth-hermes-url",
        env = "PYTH_HERMES_URL",
        default_value_t = String::from("https://hermes-beta.pyth.network")
    )]
    pub hermes_url: String,
    /// Do not push price updates to Pyth oracle if the last push was less
    /// than this long ago, even if requested.
    #[arg(
        id = "pyth-refresh-secs",
        long = "pyth-refresh-secs",
        env = "PYTH_REFRESH_SECS",
        value_parser = duration_from_secs,
        default_value = "25"
    )]
    pub refresh: Duration,
    /// Oracle ID to push price updates to.
    #[arg(
        id = "pyth-oracle-id",
        long = "pyth-oracle-id",
        env = "PYTH_ORACLE_ID",
        default_value_t = AccountId::from_str("pyth-oracle.testnet").unwrap()
    )]
    pub oracle_id: AccountId,
    /// How much gas (in units of Tgas) to attach to oracle price update calls.
    #[arg(
        id = "pyth-update-tgas",
        long = "pyth-update-tgas",
        env = "PYTH_UPDATE_TGAS",
        value_parser = gas_from_tgas,
        default_value_t = near_sdk::Gas::from_tgas(300)
    )]
    pub update_gas: near_sdk::Gas,
    /// How much NEAR to attach as a deposit to oracle price update calls.
    #[arg(
        id = "pyth-update-deposit",
        long = "pyth-update-deposit",
        env = "PYTH_UPDATE_DEPOSIT",
        default_value = "0.01 NEAR"
    )]
    pub update_deposit: NearToken,
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
}

#[derive(Args, Debug, Clone)]
#[group(required = true, multiple = true)]
pub struct Monitor {
    /// Comma-separated list of registries to query for markets to monitor.
    #[arg(
        id = "monitor-registry-id",
        long = "monitor-registry-id",
        env = "MONITOR_REGISTRY_ID",
        value_delimiter = ','
    )]
    pub registry: Vec<AccountId>,
    /// Comma-separated list of markets to monitor.
    #[arg(
        id = "monitor-market-id",
        long = "monitor-market-id",
        env = "MONITOR_MARKET_ID",
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
    /// Comma-separated list of allowed methods.
    #[arg(long, env = "ORACLE_ALLOWED_METHODS", default_values_t = vec!["update_price_feeds".to_string()], value_delimiter = ',')]
    pub oracle_allowed_methods: Vec<String>,
    /// Starting allowance in yoctoNEAR.
    #[arg(long, env = "STARTING_ALLOWANCE_YOCTO", default_value_t = NearToken::from_millinear(250))]
    pub starting_allowance_yocto: NearToken,
    /// Multiplier of minimum storage allowance to deposit on contracts, multiplied by 100.
    ///
    /// Example: a value of 200 means a multiplier of 2x of the minimum.
    #[arg(
        long,
        env = "STORAGE_DEPOSIT_MULTIPLIER_CENTS",
        default_value_t = 100u128
    )]
    pub storage_deposit_multiplier_cents: u128,
    /// Account ID of the NEAR Intents contract.
    #[arg(long, env = "INTENTS_ID")]
    pub intents_id: Option<AccountId>,
    /// Comma-separated list of sponsored methods on the intents contract.
    #[arg(long, env = "INTENTS_ALLOWED_METHODS", default_values_t = vec!["add_public_key".to_string(), "remove_public_key".to_string()], value_delimiter = ',')]
    pub intents_allowed_methods: Vec<String>,
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
    #[allow(clippy::doc_markdown)]
    /// From which origins are the payloads allowed to come?
    ///
    /// This is checked in the `clientDataJSON` field provided by WebAuthn.
    #[arg(
        id = "ua-allowed-origin",
        long = "ua-allowed-origin",
        env = "UA_ALLOWED_ORIGIN",
        value_delimiter = ','
    )]
    pub allowed_origin: Vec<String>,
    /// Account ID of the registry from which to deploy universal accounts.
    #[arg(id = "ua-registry-id", long = "ua-registry-id", env = "UA_REGISTRY_ID")]
    pub registry_id: AccountId,
    /// Version key of the universal account contract to deploy from the registry.
    #[arg(id = "ua-version-key", long = "ua-version-key", env = "UA_VERSION_KEY")]
    pub version_key: String,
    /// How much gas does it take to execute the `execute` receipt on the universal account contract?
    #[arg(
        id = "ua-execute-tgas",
        long = "ua-execute-tgas",
        env = "UA_EXECUTE_TGAS",
        default_value_t = 35
    )]
    pub execute_tgas: u64,
}

impl UniversalAccount {
    pub fn is_origin_allowed(&self, origin: Option<&str>) -> bool {
        if self.allowed_origin.is_empty() {
            true
        } else {
            origin.is_some_and(|o| self.allowed_origin.iter().any(|s| s == o))
        }
    }
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
