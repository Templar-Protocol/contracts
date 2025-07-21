use std::collections::HashMap;

use clap::ValueEnum;
use near_jsonrpc_client::{NEAR_MAINNET_RPC_URL, NEAR_TESTNET_RPC_URL};
use near_sdk::{AccountId, near};
use templar_common::borrow::BorrowPosition;

pub mod accumulator;
pub mod liquidator;
pub mod near;
pub mod swap;

type BorrowPositions = HashMap<AccountId, BorrowPosition>;

/// Helper constant for `TeraGas`.
pub const TERA_GAS: u64 = 10u64.pow(12);
/// Default gas for updating price data. 300 `TeraGas`.
pub const DEFAULT_GAS: u64 = TERA_GAS * 300;
/// One NEAR in yoctoNEAR.
pub const ONE_NEAR: u128 = 10u128.pow(24);

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
#[near(serializers = [json])]
pub enum Network {
    Mainnet,
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
    #[must_use]
    pub fn get_rpc_url(self) -> &'static str {
        match self {
            Network::Mainnet => NEAR_MAINNET_RPC_URL,
            Network::Testnet => NEAR_TESTNET_RPC_URL,
        }
    }

    #[must_use]
    pub fn get_url(self) -> &'static str {
        match self {
            Network::Mainnet => "https://hermes.pyth.network",
            Network::Testnet => "https://hermes-beta.pyth.network",
        }
    }

    #[must_use]
    #[allow(
        clippy::unwrap_used,
        reason = "We know the contract IDs are valid NEAR account IDs."
    )]
    pub fn get_oracle_account_id(self) -> AccountId {
        match self {
            Network::Mainnet => "pyth-oracle.near".parse().unwrap(),
            Network::Testnet => "pyth-oracle.testnet".parse().unwrap(),
        }
    }
}
