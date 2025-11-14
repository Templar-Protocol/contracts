use std::{collections::HashMap, sync::Arc};

use clap::Parser;
use futures::{StreamExt, TryStreamExt};
use near_crypto::{SecretKey, Signer};
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::{Action, FunctionCallAction},
    hash::CryptoHash,
    transaction::{Transaction, TransactionV0},
};
use near_sdk::{serde_json::json, AccountId};
use templar_common::market::MarketConfiguration;
use tracing::{error, info, instrument};

pub mod rpc;

use crate::rpc::{
    get_access_key_data, send_tx, serialize_and_encode, view, BorrowPositions, Network, DEFAULT_GAS,
};

#[derive(Debug, Clone, Parser)]
pub struct Args {
    /// Registries to run accumulator for
    #[arg(short, long, env = "REGISTRIES_ACCOUNT_IDS")]
    pub registries: Vec<AccountId>,
    /// Signer key to use for signing transactions
    #[arg(short = 'k', long, env = "SIGNER_KEY")]
    pub signer_key: SecretKey,
    /// Signer 'Account'
    #[arg(short, long, env = "SIGNER_ACCOUNT_ID")]
    pub signer_account: AccountId,
    /// Network to run accumulator on
    #[arg(short, long, env = "NETWORK", default_value_t = Network::Testnet)]
    pub network: Network,
    /// Timeout for transactions
    #[arg(short, long, env = "TIMEOUT", default_value_t = 60)]
    pub timeout: u64,
    /// Interval between accumulations in seconds
    #[arg(short, long, default_value_t = 600, env = "INTERVAL")]
    pub interval: u64,
    /// Interval between static accumulations in seconds
    #[arg(long, default_value_t = 86_400, env = "STATIC_INTERVAL")]
    pub static_interval: u64,
    /// Registry refresh interval in seconds
    #[arg(short, long, default_value_t = 3600, env = "REGISTRY_REFRESH_INTERVAL")]
    pub registry_refresh_interval: u64,
    /// Concurrency for accumulation tasks
    #[arg(short, long, default_value_t = 4, env = "CONCURRENCY")]
    pub concurrency: usize,
}

impl std::fmt::Display for Args {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "registries: {:?}\nsigner_account: {}\nnetwork: {}\ntimeout: {}\ninterval: {}\nstatic_interval: {}\nregistry_refresh_interval: {}\nconcurrency: {}",
            self.registries,
            self.signer_account,
            self.network,
            self.timeout,
            self.interval,
            self.static_interval,
            self.registry_refresh_interval,
            self.concurrency
        )
    }
}

pub struct Accumulator {
    client: JsonRpcClient,
    signer: Arc<Signer>,
    pub market: AccountId,
    timeout: u64,
}

impl Accumulator {
    #[must_use]
    pub fn new(
        client: JsonRpcClient,
        signer: Arc<Signer>,
        market: AccountId,
        timeout: u64,
    ) -> Self {
        Self {
            client,
            signer,
            market,
            timeout,
        }
    }

    fn create_tx(
        &self,
        borrow: &AccountId,
        nonce: u64,
        block_hash: CryptoHash,
        method_name: String,
    ) -> Transaction {
        Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: self.market.clone(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name,
                args: serialize_and_encode(json!({
                    "account_id": borrow,
                })),
                gas: DEFAULT_GAS,
                deposit: 0,
            }))],
        })
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn accumulate(&self, borrow: AccountId, method: &str) -> anyhow::Result<()> {
        info!("Starting accumulation for market: {}", self.market);

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        let accumulate_tx = self.create_tx(&borrow, nonce, block_hash, method.to_owned());

        match send_tx(&self.client, &self.signer, self.timeout, accumulate_tx).await {
            Ok(_) => {
                info!("Accumulation successful");
            }
            Err(e) => {
                error!("Accumulation failed: {e}");
            }
        }

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_borrows(&self) -> anyhow::Result<BorrowPositions> {
        let mut all_positions: BorrowPositions = HashMap::new();

        let page_size = 100;
        let mut current_offset = 0;
        let mut params = json!({
            "offset": current_offset,
            "count": page_size,
        });

        while let Ok(page) = view::<BorrowPositions>(
            &self.client,
            self.market.clone(),
            "list_borrow_positions",
            params.clone(),
        )
        .await
        {
            let fetched = page.len();
            all_positions.extend(page);
            current_offset += page_size;
            params["offset"] = current_offset.into();

            if fetched < page_size {
                break;
            }
        }

        Ok(all_positions)
    }

    #[instrument(skip(self), level = "info")]
    pub async fn run_borrow_accumulations(&self, concurrency: usize) -> anyhow::Result<()> {
        let borrows = self.get_borrows().await?;

        if borrows.is_empty() {
            return Ok(());
        }

        futures::stream::iter(borrows)
            .map(|(account_id, _)| async move { self.accumulate(account_id, "apply_interest").await })
            .buffer_unordered(concurrency)
            .try_for_each(|_result| async { Ok(()) })
            .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "info")]
    pub async fn run_static_accumulations(&self, concurrency: usize) -> anyhow::Result<()> {
        let static_accounts = self.get_static_accounts().await?;

        if static_accounts.is_empty() {
            return Ok(());
        }

        futures::stream::iter(static_accounts)
            .map(|account_id| async move {
                self.accumulate(account_id, "accumulate_static_yield").await
            })
            .buffer_unordered(concurrency)
            .try_for_each(|_result| async { Ok(()) })
            .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_static_accounts(&self) -> anyhow::Result<Vec<AccountId>> {
        let configuration: MarketConfiguration = view(
            &self.client,
            self.market.clone(),
            "get_configuration",
            json!({}),
        )
        .await?;

        Ok(configuration
            .yield_weights
            .r#static
            .keys()
            .cloned()
            .collect())
    }
}
