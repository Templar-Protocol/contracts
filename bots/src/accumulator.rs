use std::collections::HashMap;

use clap::Parser;
use futures::{StreamExt, TryStreamExt};
use near_crypto::{InMemorySigner, SecretKey};
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::{Action, FunctionCallAction},
    hash::CryptoHash,
    transaction::{Transaction, TransactionV0},
};
use near_sdk::{AccountId, serde_json::json};
use tracing::{error, info, instrument};

use crate::{
    BorrowPositions, DEFAULT_GAS, Network,
    near::{get_access_key_data, send_tx, serialize_and_encode, view},
};

#[derive(Debug, Clone, Parser)]
pub struct Args {
    /// Market to run accumulator for
    #[arg(short, long, env = "MARKET_ACCOUNT_ID")]
    pub markets: Vec<AccountId>,
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
    #[arg(short, long, default_value = "60", env = "INTERVAL")]
    pub interval: u64,
    /// Concurrency for accumulation tasks
    #[arg(short, long, default_value = "10", env = "CONCURRENCY")]
    pub concurrency: usize,
}

pub struct Accumulator {
    client: JsonRpcClient,
    signer: InMemorySigner,
    pub market: AccountId,
    timeout: u64,
}

impl Accumulator {
    #[must_use]
    pub fn new(
        client: JsonRpcClient,
        signer: InMemorySigner,
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

    fn create_accumulate_tx(
        &self,
        borrow: &AccountId,
        nonce: u64,
        block_hash: CryptoHash,
    ) -> Transaction {
        Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: self.market.clone(),
            block_hash,
            signer_id: self.signer.account_id.clone(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: "apply_interest".to_string(),
                args: serialize_and_encode(json!({
                    "account_id": borrow,
                })),
                gas: DEFAULT_GAS,
                deposit: 0,
            }))],
        })
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn accumulate(&self, borrow: AccountId) -> anyhow::Result<()> {
        info!("Starting accumulation for market: {}", self.market);

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        let apply_interest_tx = self.create_accumulate_tx(&borrow, nonce, block_hash);

        match send_tx(&self.client, &self.signer, self.timeout, apply_interest_tx).await {
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
    pub async fn run_accumulations(&self, concurrency: usize) -> anyhow::Result<()> {
        let borrows = self.get_borrows().await?;

        if borrows.is_empty() {
            return Ok(());
        }

        futures::stream::iter(borrows)
            .map(|(account_id, _)| async move { self.accumulate(account_id).await })
            .buffer_unordered(concurrency)
            .try_for_each(|_result| async { Ok(()) })
            .await?;

        Ok(())
    }

    #[instrument(level = "debug")]
    pub fn setup_accumulators(args: &Args) -> anyhow::Result<Vec<Self>> {
        let client = JsonRpcClient::connect(args.network.rpc_url());
        let signer =
            InMemorySigner::from_secret_key(args.signer_account.clone(), args.signer_key.clone());

        Ok(args
            .markets
            .iter()
            .map(|market| Self::new(client.clone(), signer.clone(), market.clone(), args.timeout))
            .collect())
    }
}
