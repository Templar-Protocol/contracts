//! NEAR network selection for off-chain gateway consumers.

use std::fmt;

/// A NEAR network, used by off-chain consumers (CLIs, bots, services) to pick
/// the default RPC endpoint when constructing a [`crate::Client`].
///
/// This is the shared home for the `Network` enum that off-chain tools/services
/// previously each defined. Under the `clap` feature it derives
/// [`clap::ValueEnum`] so binaries can accept it directly as a CLI/env argument.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Network {
    /// NEAR mainnet.
    Mainnet,
    /// NEAR testnet.
    #[default]
    Testnet,
}

impl Network {
    /// The default public RPC URL for this network.
    ///
    /// Consumers can override this (e.g. with a `--rpc-url` flag) before
    /// building a [`near_api::NetworkConfig`].
    #[must_use]
    pub fn rpc_url(self) -> &'static str {
        match self {
            Network::Mainnet => "https://rpc.mainnet.fastnear.com",
            Network::Testnet => "https://rpc.testnet.fastnear.com",
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Network::Mainnet => "mainnet",
            Network::Testnet => "testnet",
        })
    }
}
