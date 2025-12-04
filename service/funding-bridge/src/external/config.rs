//! Chain configuration for external blockchains

use std::collections::HashMap;

/// Configuration for an EVM chain
#[derive(Debug, Clone)]
pub struct EvmChainConfig {
    /// Chain identifier (e.g., "eth:1", "eth:42161")
    pub chain_id: String,
    /// Human-readable name
    pub name: String,
    /// Default RPC URL
    pub rpc_url: String,
    /// Native chain ID (e.g., 1 for Ethereum mainnet)
    pub native_chain_id: u64,
    /// Token contract addresses: asset symbol -> contract address
    pub token_addresses: HashMap<String, String>,
    /// Token decimals: asset symbol -> decimals (default 18)
    pub token_decimals: HashMap<String, u8>,
}

impl EvmChainConfig {
    /// Create Ethereum mainnet configuration
    pub fn ethereum() -> Self {
        let mut token_addresses = HashMap::new();
        token_addresses.insert(
            "USDC".to_string(),
            "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string(),
        );
        token_addresses.insert(
            "USDT".to_string(),
            "0xdAC17F958D2ee523a2206206994597C13D831ec7".to_string(),
        );

        let mut token_decimals = HashMap::new();
        token_decimals.insert("USDC".to_string(), 6);
        token_decimals.insert("USDT".to_string(), 6);

        Self {
            chain_id: "eth:1".to_string(),
            name: "Ethereum".to_string(),
            rpc_url: "https://eth.llamarpc.com".to_string(),
            native_chain_id: 1,
            token_addresses,
            token_decimals,
        }
    }

    /// Create Arbitrum One configuration
    pub fn arbitrum() -> Self {
        let mut token_addresses = HashMap::new();
        token_addresses.insert(
            "USDC".to_string(),
            "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".to_string(), // Native USDC
        );
        token_addresses.insert(
            "USDT".to_string(),
            "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9".to_string(),
        );

        let mut token_decimals = HashMap::new();
        token_decimals.insert("USDC".to_string(), 6);
        token_decimals.insert("USDT".to_string(), 6);

        Self {
            chain_id: "eth:42161".to_string(),
            name: "Arbitrum".to_string(),
            rpc_url: "https://arb1.arbitrum.io/rpc".to_string(),
            native_chain_id: 42161,
            token_addresses,
            token_decimals,
        }
    }

    /// Create Base configuration
    pub fn base() -> Self {
        let mut token_addresses = HashMap::new();
        token_addresses.insert(
            "USDC".to_string(),
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_string(), // Native USDC
        );

        let mut token_decimals = HashMap::new();
        token_decimals.insert("USDC".to_string(), 6);

        Self {
            chain_id: "eth:8453".to_string(),
            name: "Base".to_string(),
            rpc_url: "https://mainnet.base.org".to_string(),
            native_chain_id: 8453,
            token_addresses,
            token_decimals,
        }
    }

    /// Create Optimism configuration
    pub fn optimism() -> Self {
        let mut token_addresses = HashMap::new();
        token_addresses.insert(
            "USDC".to_string(),
            "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85".to_string(), // Native USDC
        );

        let mut token_decimals = HashMap::new();
        token_decimals.insert("USDC".to_string(), 6);

        Self {
            chain_id: "eth:10".to_string(),
            name: "Optimism".to_string(),
            rpc_url: "https://mainnet.optimism.io".to_string(),
            native_chain_id: 10,
            token_addresses,
            token_decimals,
        }
    }

    /// Create Polygon configuration
    pub fn polygon() -> Self {
        let mut token_addresses = HashMap::new();
        token_addresses.insert(
            "USDC".to_string(),
            "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359".to_string(), // Native USDC
        );
        token_addresses.insert(
            "USDT".to_string(),
            "0xc2132D05D31c914a87C6611C10748AEb04B58e8F".to_string(),
        );

        let mut token_decimals = HashMap::new();
        token_decimals.insert("USDC".to_string(), 6);
        token_decimals.insert("USDT".to_string(), 6);

        Self {
            chain_id: "eth:137".to_string(),
            name: "Polygon".to_string(),
            rpc_url: "https://polygon-rpc.com".to_string(),
            native_chain_id: 137,
            token_addresses,
            token_decimals,
        }
    }

    /// Get all default EVM chain configurations
    pub fn all_defaults() -> Vec<Self> {
        vec![
            Self::ethereum(),
            Self::arbitrum(),
            Self::base(),
            Self::optimism(),
            Self::polygon(),
        ]
    }

    /// Get token address for asset
    pub fn get_token_address(&self, asset: &str) -> Option<&str> {
        self.token_addresses
            .get(&asset.to_uppercase())
            .map(|s| s.as_str())
    }

    /// Get token decimals for asset
    pub fn get_token_decimals(&self, asset: &str) -> u8 {
        self.token_decimals
            .get(&asset.to_uppercase())
            .copied()
            .unwrap_or(18)
    }
}
