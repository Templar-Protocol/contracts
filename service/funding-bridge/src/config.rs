//! Configuration management for funding-bridge service
//!
//! Supports CLI arguments and environment variables via clap.

use std::{env, fs};

use clap::Parser;
use near_crypto::SecretKey;
use near_primitives::types::AccountId;

use crate::error::{FundingError, FundingResult};
use crate::rpc::Network;

/// Funding Bridge Service - NEAR treasury with cross-chain bridge operations
#[derive(Parser, Debug, Clone)]
#[command(name = "funding-bridge")]
#[command(
    about = "NEAR treasury management with cross-chain deposits/withdrawals via NEAR Intents Bridge"
)]
#[command(version)]
pub struct Args {
    /// HTTP server port
    #[arg(long, env = "PORT", default_value_t = 3000)]
    pub port: u16,

    /// NEAR network
    #[arg(long, env = "NETWORK", default_value = "mainnet")]
    pub network: Network,

    /// NEAR Intents Bridge API endpoint
    #[arg(
        long,
        env = "BRIDGE_API_URL",
        default_value = "https://bridge.chaindefuser.com/rpc"
    )]
    pub bridge_api_url: String,

    /// Dry run mode (log only, no transactions)
    #[arg(long, env = "DRY_RUN", default_value_t = false)]
    pub dry_run: bool,

    // === NEAR Treasury (required) ===
    /// NEAR treasury account ID
    #[arg(long, env = "NEAR_TREASURY_ACCOUNT")]
    pub near_treasury_account: Option<AccountId>,

    /// NEAR treasury signer key
    #[arg(long)]
    pub near_treasury_key: Option<SecretKey>,

    /// NEAR RPC URL (optional, uses network default if not specified)
    #[arg(long, env = "NEAR_RPC_URL")]
    pub near_rpc_url: Option<String>,

    // === Ethereum Wallet (for automated deposits) ===
    /// Ethereum private key (hex, with or without 0x prefix)
    #[arg(long, env = "ETH_PRIVATE_KEY")]
    pub eth_private_key: Option<String>,

    /// Ethereum RPC URL
    #[arg(long, env = "ETH_RPC_URL", default_value = "https://eth.llamarpc.com")]
    pub eth_rpc_url: String,

    // === Solana Wallet (for automated deposits) ===
    /// Solana private key (base58 encoded)
    #[arg(long, env = "SOLANA_PRIVATE_KEY")]
    pub solana_private_key: Option<String>,

    /// Solana RPC URL
    #[arg(
        long,
        env = "SOLANA_RPC_URL",
        default_value = "https://api.mainnet-beta.solana.com"
    )]
    pub solana_rpc_url: String,

    // === Stellar Wallet (for automated deposits) ===
    /// Stellar secret key (S...)
    #[arg(long, env = "STELLAR_SECRET_KEY")]
    pub stellar_secret_key: Option<String>,

    /// Stellar Horizon URL
    #[arg(
        long,
        env = "STELLAR_HORIZON_URL",
        default_value = "https://horizon.stellar.org"
    )]
    pub stellar_horizon_url: String,

    // === Withdrawal Destinations (required for withdrawals) ===
    /// Ethereum withdrawal destination address
    #[arg(long, env = "ETH_WITHDRAW_ADDRESS")]
    pub eth_withdraw_address: Option<String>,

    /// Arbitrum withdrawal destination address
    #[arg(long, env = "ARBITRUM_WITHDRAW_ADDRESS")]
    pub arbitrum_withdraw_address: Option<String>,

    /// Base withdrawal destination address
    #[arg(long, env = "BASE_WITHDRAW_ADDRESS")]
    pub base_withdraw_address: Option<String>,

    /// Optimism withdrawal destination address
    #[arg(long, env = "OPTIMISM_WITHDRAW_ADDRESS")]
    pub optimism_withdraw_address: Option<String>,

    /// Polygon withdrawal destination address
    #[arg(long, env = "POLYGON_WITHDRAW_ADDRESS")]
    pub polygon_withdraw_address: Option<String>,

    /// Solana withdrawal destination address
    #[arg(long, env = "SOLANA_WITHDRAW_ADDRESS")]
    pub solana_withdraw_address: Option<String>,

    /// Stellar withdrawal destination address
    #[arg(long, env = "STELLAR_WITHDRAW_ADDRESS")]
    pub stellar_withdraw_address: Option<String>,
}

impl Args {
    /// Parse command-line arguments and resolve secret-backed key inputs.
    pub fn parse_args() -> FundingResult<Self> {
        let mut args = Self::parse();
        args.near_treasury_key = resolve_secret_key(
            args.near_treasury_key.take(),
            "NEAR_TREASURY_KEY",
            "NEAR_TREASURY_KEY_FILE",
        )?;
        Ok(args)
    }

    /// Validate configuration
    pub fn validate(&self) -> FundingResult<()> {
        // Validate NEAR treasury config
        if self.near_treasury_account.is_none() {
            return Err(FundingError::ConfigError(
                "NEAR_TREASURY_ACCOUNT is required".to_string(),
            ));
        }
        if self.near_treasury_key.is_none() {
            return Err(FundingError::ConfigError(
                "NEAR_TREASURY_KEY or NEAR_TREASURY_KEY_FILE is required".to_string(),
            ));
        }

        Ok(())
    }

    /// Get withdrawal destination address for a chain
    pub fn get_withdraw_address(&self, chain: &str) -> Option<String> {
        match chain {
            "ethereum" | "eth" | "eth:1" => self.eth_withdraw_address.clone(),
            "arbitrum" | "arb" | "eth:42161" => self.arbitrum_withdraw_address.clone(),
            "base" | "eth:8453" => self.base_withdraw_address.clone(),
            "optimism" | "op" | "eth:10" => self.optimism_withdraw_address.clone(),
            "polygon" | "matic" | "eth:137" => self.polygon_withdraw_address.clone(),
            "solana" | "sol" | "sol:mainnet" => self.solana_withdraw_address.clone(),
            "stellar" | "stellar:mainnet" | "stellar:testnet" => {
                self.stellar_withdraw_address.clone()
            }
            "near" | "near:mainnet" | "near:testnet" => {
                // For NEAR, use NEAR_ACCOUNT (same account for deposits/withdrawals)
                std::env::var("NEAR_ACCOUNT").ok()
            }
            _ => None,
        }
    }

    /// Get NEAR RPC URL based on network
    pub fn get_near_treasury_rpc_url(&self) -> String {
        self.near_rpc_url
            .clone()
            .unwrap_or_else(|| self.network.rpc_url().to_string())
    }
}

fn resolve_secret_key(
    cli_value: Option<SecretKey>,
    env_var: &str,
    file_env_var: &str,
) -> FundingResult<Option<SecretKey>> {
    if cli_value.is_some() {
        return Ok(cli_value);
    }

    if let Some(path) = read_env_value(file_env_var) {
        let key = fs::read_to_string(&path).map_err(|err| {
            FundingError::ConfigError(format!("Failed to read {} from {}: {}", env_var, path, err))
        })?;
        let key = key.trim();
        if key.is_empty() {
            return Err(FundingError::ConfigError(format!(
                "{} points to an empty file: {}",
                file_env_var, path
            )));
        }

        return key.parse::<SecretKey>().map(Some).map_err(|err| {
            FundingError::ConfigError(format!(
                "Failed to parse {} from {}: {}",
                env_var, path, err
            ))
        });
    }

    read_env_value(env_var)
        .map(|key| {
            key.parse::<SecretKey>().map_err(|err| {
                FundingError::ConfigError(format!("Failed to parse {}: {}", env_var, err))
            })
        })
        .transpose()
}

fn read_env_value(name: &str) -> Option<String> {
    let value = env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn create_valid_config() -> Args {
        Args {
            port: 3000,
            network: Network::Testnet,
            bridge_api_url: "https://bridge.chaindefuser.com/rpc".to_string(),
            dry_run: false,
            near_treasury_account: Some(AccountId::from_str("treasury.near").unwrap()),
            near_treasury_key: Some(SecretKey::from_random(near_crypto::KeyType::ED25519)),
            near_rpc_url: None,
            eth_private_key: None,
            eth_rpc_url: "https://eth.llamarpc.com".to_string(),
            solana_private_key: None,
            solana_rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
            eth_withdraw_address: None,
            arbitrum_withdraw_address: None,
            base_withdraw_address: None,
            optimism_withdraw_address: None,
            polygon_withdraw_address: None,
            solana_withdraw_address: None,
            stellar_secret_key: None,
            stellar_horizon_url: "https://horizon.stellar.org".to_string(),
            stellar_withdraw_address: None,
        }
    }

    #[test]
    fn test_valid_config() {
        let config = create_valid_config();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_near_missing_account() {
        let mut config = create_valid_config();
        config.near_treasury_account = None;

        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(FundingError::ConfigError(msg)) => {
                assert!(msg.contains("NEAR_TREASURY_ACCOUNT"));
            }
            _ => panic!("Expected ConfigError"),
        }
    }

    #[test]
    fn test_near_missing_signer_key() {
        let mut config = create_valid_config();
        config.near_treasury_key = None;

        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(FundingError::ConfigError(msg)) => {
                assert!(msg.contains("NEAR_TREASURY_KEY"));
            }
            _ => panic!("Expected ConfigError"),
        }
    }

    #[test]
    fn test_resolve_secret_key_from_file_env() {
        let key = SecretKey::from_random(near_crypto::KeyType::ED25519);
        let file_path = unique_temp_file_path("funding-bridge-near-key");
        let original_key = std::env::var("NEAR_TREASURY_KEY").ok();
        let original_key_file = std::env::var("NEAR_TREASURY_KEY_FILE").ok();

        fs::write(&file_path, key.to_string()).unwrap();
        std::env::remove_var("NEAR_TREASURY_KEY");
        std::env::set_var("NEAR_TREASURY_KEY_FILE", &file_path);

        let resolved =
            resolve_secret_key(None, "NEAR_TREASURY_KEY", "NEAR_TREASURY_KEY_FILE").unwrap();
        assert_eq!(resolved, Some(key.clone()));

        restore_env("NEAR_TREASURY_KEY", original_key);
        restore_env("NEAR_TREASURY_KEY_FILE", original_key_file);
        fs::remove_file(file_path).unwrap();
    }

    #[test]
    fn test_resolve_secret_key_ignores_blank_env() {
        let original_key = std::env::var("NEAR_TREASURY_KEY").ok();
        let original_key_file = std::env::var("NEAR_TREASURY_KEY_FILE").ok();

        std::env::set_var("NEAR_TREASURY_KEY", "");
        std::env::set_var("NEAR_TREASURY_KEY_FILE", "   ");

        let resolved =
            resolve_secret_key(None, "NEAR_TREASURY_KEY", "NEAR_TREASURY_KEY_FILE").unwrap();
        assert!(resolved.is_none());

        restore_env("NEAR_TREASURY_KEY", original_key);
        restore_env("NEAR_TREASURY_KEY_FILE", original_key_file);
    }

    #[test]
    fn test_get_near_treasury_rpc_url_mainnet() {
        let mut config = create_valid_config();
        config.network = Network::Mainnet;
        config.near_rpc_url = None;

        assert_eq!(
            config.get_near_treasury_rpc_url(),
            "https://free.rpc.fastnear.com"
        );
    }

    #[test]
    fn test_get_near_treasury_rpc_url_testnet() {
        let mut config = create_valid_config();
        config.network = Network::Testnet;
        config.near_rpc_url = None;

        assert_eq!(
            config.get_near_treasury_rpc_url(),
            "https://rpc.testnet.near.org"
        );
    }

    #[test]
    fn test_get_near_treasury_rpc_url_custom() {
        let mut config = create_valid_config();
        config.near_rpc_url = Some("https://custom.rpc.near.org".to_string());

        assert_eq!(
            config.get_near_treasury_rpc_url(),
            "https://custom.rpc.near.org"
        );
    }

    #[test]
    fn test_dry_run_flag() {
        let mut config = create_valid_config();
        assert!(!config.dry_run);

        config.dry_run = true;
        assert!(config.dry_run);
        assert!(config.validate().is_ok());
    }

    fn unique_temp_file_path(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}.txt"))
    }

    fn restore_env(name: &str, value: Option<String>) {
        if let Some(value) = value {
            std::env::set_var(name, value);
        } else {
            std::env::remove_var(name);
        }
    }
}
