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
    /// Refresh the cached gas price after X seconds.
    #[arg(long, env = "CACHE_GAS_PRICE_SECS", default_value_t = 600)]
    pub cache_gas_price_secs: u64,
    /// Refresh a cached nonce after X seconds.
    #[arg(long, env = "CACHE_NONCE_SECS", default_value_t = 60)]
    pub cache_nonce_secs: u64,
    /// Broom batch size.
    #[arg(long, env = "BROOM_BATCH_SIZE", default_value_t = 16)]
    pub broom_batch_size: u32,
    /// Broom interval in seconds.
    #[arg(long, env = "BROOM_INTERVAL_SECS", default_value_t = 300)]
    pub broom_interval_secs: u64,

    #[clap(flatten)]
    pub ua: UniversalAccount,
}

#[derive(Args, Debug, Clone)]
#[group(required = true, multiple = true)]
pub struct Monitor {
    /// Comma-separated list of registries to query for markets to monitor.
    #[arg(long, env = "REGISTRY", value_delimiter = ',')]
    pub registry: Vec<AccountId>,
    /// Comma-separated list of markets to monitor.
    #[arg(long, env = "MARKET", value_delimiter = ',')]
    pub market: Vec<AccountId>,
}

#[derive(Args, Debug, Clone)]
pub struct Relay {
    /// Account ID of the NEAR account that the relayer controls.
    #[arg(long = "relay-account-id", env = "RELAY_ACCOUNT_ID")]
    pub account_id: AccountId,
    /// Comma-separated list of private keys to use to sign transactions for the account that the relayer controls.
    #[arg(
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
    #[arg(long = "ua-account-id", env = "UA_ACCOUNT_ID")]
    pub account_id: AccountId,
    /// Comma-separated list of private keys to use to sign universal account creation transactions.
    #[arg(long = "ua-secret-key", env = "UA_SECRET_KEY", value_delimiter = ',')]
    pub secret_key: Vec<SecretKey>,
    /// How difficult should the proof-of-work for universal account creation be?
    ///
    /// iterations ~ 2^difficulty
    #[arg(
        long = "ua-pow-difficulty",
        env = "UA_POW_DIFFICULTY",
        default_value_t = 17
    )]
    pub pow_difficulty: usize,
    /// How fresh must the universal account creation signature be?
    ///
    /// Based on the block hash referenced in the creation request.
    #[arg(long = "ua-blockref-max-age-ms", env = "UA_BLOCKREF_MAX_AGE_MS", default_value_t = 1000 * 60 * 10 /* 10 minutes */)]
    pub blockref_max_age_ms: u64,
    /// Account ID of the registry from which to deploy universal accounts.
    #[arg(long = "ua-registry-id", env = "UA_REGISTRY_ID")]
    pub registry_id: AccountId,
    /// Version key of the universal account contract to deploy from the registry.
    #[arg(long = "ua-version-key", env = "UA_VERSION_KEY")]
    pub version_key: String,
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
