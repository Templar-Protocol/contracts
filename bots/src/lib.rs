use std::collections::HashMap;

use clap::ValueEnum;
use near_jsonrpc_client::{NEAR_MAINNET_RPC_URL, NEAR_TESTNET_RPC_URL};
use near_sdk::{near, AccountId, Gas};
use templar_common::borrow::BorrowPosition;

pub mod accumulator;
pub mod liquidator;
pub mod near;
pub mod swap;
pub mod types;

type BorrowPositions = HashMap<AccountId, BorrowPosition>;

/// Default gas for updating price data. 300 `TeraGas`.
pub const DEFAULT_GAS: u64 = Gas::from_tgas(300).as_gas();

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
    pub fn rpc_url(&self) -> &str {
        match self {
            Network::Mainnet => NEAR_MAINNET_RPC_URL,
            Network::Testnet => NEAR_TESTNET_RPC_URL,
        }
    }
}
