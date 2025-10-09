use std::collections::HashMap;

use clap::ValueEnum;
use futures::{StreamExt, TryStreamExt};
use near::{view, RpcResult};
use near_jsonrpc_client::{JsonRpcClient, NEAR_MAINNET_RPC_URL, NEAR_TESTNET_RPC_URL};
use near_sdk::{near, serde_json::json, AccountId, Gas};
use templar_common::borrow::BorrowPosition;
use tracing::instrument;

pub mod accumulator;
pub mod liquidator;
pub mod near;
pub mod swap;

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

#[instrument(skip(client), level = "debug")]
pub async fn list_deployments(
    client: &JsonRpcClient,
    registry: AccountId,
    count: Option<u32>,
    offset: Option<u32>,
) -> RpcResult<Vec<AccountId>> {
    let mut all_deployments = Vec::new();
    let page_size = 500;
    let mut current_offset = 0;

    loop {
        let params = json!({
            "offset": current_offset,
            "count": page_size,
        });

        let page =
            view::<Vec<AccountId>>(client, registry.clone(), "list_deployments", params).await?;

        let fetched = page.len();

        if fetched == 0 {
            break;
        }

        all_deployments.extend(page);
        current_offset += fetched;

        if fetched < page_size {
            break;
        }
    }

    Ok(all_deployments)
}

#[instrument(skip(client), level = "debug")]
pub async fn list_all_deployments(
    client: JsonRpcClient,
    registries: Vec<AccountId>,
    concurrency: usize,
) -> RpcResult<Vec<AccountId>> {
    let all_markets: Vec<AccountId> = futures::stream::iter(registries)
        .map(|registry| {
            let client = client.clone();
            async move { list_deployments(&client, registry, None, None).await }
        })
        .buffer_unordered(concurrency)
        .try_concat()
        .await?;

    Ok(all_markets)
}
