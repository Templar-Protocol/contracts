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
    /// Account ID of the NEAR account that the relayer controls.
    #[arg(short, long, env = "ACCOUNT_ID")]
    pub account_id: AccountId,
    /// Comma-separated list of private keys to use to sign transactions for the account that the relayer controls.
    #[arg(short = 'k', long, env = "SECRET_KEY", value_delimiter = ',')]
    pub secret_key: Vec<SecretKey>,
    /// Comma-separated list of allowed methods.
    #[arg(long, env = "ALLOWED_METHODS", default_values_t = default_allowed_methods(), value_delimiter = ',')]
    pub allowed_methods: Vec<String>,
    /// Starting allowance in yoctoNEAR.
    #[arg(long, env = "STARTING_ALLOWANCE_YOCTO", default_value_t = NearToken::from_millinear(250))]
    pub starting_allowance_yocto: NearToken,
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
