use near_sdk::near;

/// Network configuration for NEAR
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
#[near(serializers = [json])]
pub enum Network {
    /// NEAR mainnet
    Mainnet,
    /// NEAR testnet (default)
    #[default]
    Testnet,
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Network::Mainnet => "mainnet",
                Network::Testnet => "testnet",
            }
        )
    }
}

impl Network {
    /// Get the RPC URL for this network
    #[must_use]
    pub fn rpc_url(&self) -> &str {
        use near_jsonrpc_client::{NEAR_MAINNET_RPC_URL, NEAR_TESTNET_RPC_URL};

        match self {
            Network::Mainnet => NEAR_MAINNET_RPC_URL,
            Network::Testnet => NEAR_TESTNET_RPC_URL,
        }
    }
}
