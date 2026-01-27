//! RPC utilities for NEAR blockchain operations

use serde::{Deserialize, Serialize};

/// Network configuration for NEAR
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
pub enum Network {
    /// NEAR Mainnet
    Mainnet,
    /// NEAR Testnet (default)
    #[default]
    Testnet,
}

impl Network {
    /// Get default RPC URL for network
    pub fn rpc_url(&self) -> &'static str {
        match self {
            Network::Mainnet => "https://free.rpc.fastnear.com",
            Network::Testnet => "https://rpc.testnet.near.org",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_rpc_url_mainnet() {
        assert_eq!(Network::Mainnet.rpc_url(), "https://free.rpc.fastnear.com");
    }

    #[test]
    fn test_network_rpc_url_testnet() {
        assert_eq!(Network::Testnet.rpc_url(), "https://rpc.testnet.near.org");
    }

    #[test]
    fn test_network_default() {
        let network = Network::default();
        assert_eq!(network, Network::Testnet);
    }
}
