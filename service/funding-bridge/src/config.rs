//! Configuration management for funding-bridge service
//!
//! Supports CLI arguments and environment variables via clap.

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

    /// NEAR network (mainnet or testnet)
    #[arg(long, env = "NETWORK", default_value = "testnet")]
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
    /// Enable NEAR treasury (must be true)
    #[arg(long, env = "NEAR_ENABLED", default_value_t = false)]
    pub near_enabled: bool,

    /// NEAR treasury account ID
    #[arg(long, env = "NEAR_TREASURY_ACCOUNT")]
    pub near_treasury_account: Option<AccountId>,

    /// NEAR signer key
    #[arg(long, env = "NEAR_SIGNER_KEY")]
    pub near_signer_key: Option<SecretKey>,

    /// NEAR RPC URL (optional, uses network default if not specified)
    #[arg(long, env = "NEAR_RPC_URL")]
    pub near_rpc_url: Option<String>,

    /// NEAR routing priority (0 = highest)
    #[arg(long, env = "NEAR_PRIORITY", default_value_t = 0)]
    pub near_priority: u8,

    // === Ethereum Wallet (for automated deposits) ===
    /// Ethereum private key (hex, with or without 0x prefix)
    #[arg(long, env = "ETH_PRIVATE_KEY")]
    pub eth_private_key: Option<String>,

    /// Ethereum RPC URL
    #[arg(long, env = "ETH_RPC_URL", default_value = "https://eth.llamarpc.com")]
    pub eth_rpc_url: String,

    // === Solana Wallet (for automated deposits) ===
    /// Solana private key (base58 encoded)
    #[arg(long, env = "SOL_PRIVATE_KEY")]
    pub sol_private_key: Option<String>,

    /// Solana RPC URL
    #[arg(
        long,
        env = "SOL_RPC_URL",
        default_value = "https://api.mainnet-beta.solana.com"
    )]
    pub sol_rpc_url: String,
}

impl Args {
    /// Validate configuration
    pub fn validate(&self) -> FundingResult<()> {
        // NEAR treasury must be enabled
        if !self.near_enabled {
            return Err(FundingError::ConfigError(
                "NEAR treasury must be enabled (--near-enabled)".to_string(),
            ));
        }

        // Validate NEAR config
        if self.near_treasury_account.is_none() {
            return Err(FundingError::ConfigError(
                "NEAR_TREASURY_ACCOUNT required when NEAR_ENABLED=true".to_string(),
            ));
        }
        if self.near_signer_key.is_none() {
            return Err(FundingError::ConfigError(
                "NEAR_SIGNER_KEY required when NEAR_ENABLED=true".to_string(),
            ));
        }

        Ok(())
    }

    /// Get NEAR RPC URL based on network
    pub fn get_near_rpc_url(&self) -> String {
        self.near_rpc_url
            .clone()
            .unwrap_or_else(|| self.network.rpc_url().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn create_valid_config() -> Args {
        Args {
            port: 3000,
            network: Network::Testnet,
            bridge_api_url: "https://bridge.chaindefuser.com/rpc".to_string(),
            dry_run: false,
            near_enabled: true,
            near_treasury_account: Some(AccountId::from_str("treasury.near").unwrap()),
            near_signer_key: Some(SecretKey::from_random(near_crypto::KeyType::ED25519)),
            near_rpc_url: None,
            near_priority: 0,
            eth_private_key: None,
            eth_rpc_url: "https://eth.llamarpc.com".to_string(),
            sol_private_key: None,
            sol_rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
        }
    }

    #[test]
    fn test_valid_config() {
        let config = create_valid_config();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_near_not_enabled() {
        let mut config = create_valid_config();
        config.near_enabled = false;

        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(FundingError::ConfigError(msg)) => {
                assert!(msg.contains("NEAR treasury must be enabled"));
            }
            _ => panic!("Expected ConfigError"),
        }
    }

    #[test]
    fn test_near_enabled_missing_account() {
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
    fn test_near_enabled_missing_signer_key() {
        let mut config = create_valid_config();
        config.near_signer_key = None;

        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(FundingError::ConfigError(msg)) => {
                assert!(msg.contains("NEAR_SIGNER_KEY"));
            }
            _ => panic!("Expected ConfigError"),
        }
    }

    #[test]
    fn test_get_near_rpc_url_mainnet() {
        let mut config = create_valid_config();
        config.network = Network::Mainnet;
        config.near_rpc_url = None;

        assert_eq!(config.get_near_rpc_url(), "https://rpc.mainnet.near.org");
    }

    #[test]
    fn test_get_near_rpc_url_testnet() {
        let mut config = create_valid_config();
        config.network = Network::Testnet;
        config.near_rpc_url = None;

        assert_eq!(config.get_near_rpc_url(), "https://rpc.testnet.near.org");
    }

    #[test]
    fn test_get_near_rpc_url_custom() {
        let mut config = create_valid_config();
        config.near_rpc_url = Some("https://custom.rpc.near.org".to_string());

        assert_eq!(config.get_near_rpc_url(), "https://custom.rpc.near.org");
    }

    #[test]
    fn test_dry_run_flag() {
        let mut config = create_valid_config();
        assert!(!config.dry_run);

        config.dry_run = true;
        assert!(config.dry_run);
        assert!(config.validate().is_ok());
    }
}
