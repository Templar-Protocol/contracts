use clap::Parser;
use near_crypto::{InMemorySigner, SecretKey};
use near_sdk::{AccountId, serde_json::json};
use std::sync::Arc;

use tracing::{error, info, instrument};

use near_jsonrpc_client::JsonRpcClient;

use crate::{Network, near::call_apply_interest};

#[derive(Debug, Clone, Parser)]
pub struct Args {
    /// Market to run accumulator for
    #[arg(short, long, env = "MARKET_ACCOUNT_ID")]
    pub markets: Vec<AccountId>,
    /// Signer key to use for signing transactions
    #[arg(short, long, env = "SIGNER_KEY")]
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

    #[instrument(skip(self), level = "debug")]
    pub async fn accumulate(&self, borrow: AccountId) -> anyhow::Result<()> {
        info!("Starting accumulation for market: {}", self.market);

        match call_apply_interest(
            &self.client,
            &self.signer,
            self.market.clone(),
            json!({
                "account_id": borrow,
            }),
            self.timeout,
        )
        .await
        {
            Ok(_) => {
                info!("Accumulation successful");
            }
            Err(e) => {
                error!("Accumulation failed: {e}");
            }
        }

        Ok(())
    }
}

#[instrument(level = "debug")]
pub fn setup_accumulator(args: &Args) -> anyhow::Result<(JsonRpcClient, Vec<Arc<Accumulator>>)> {
    let client = JsonRpcClient::connect(args.network.get_rpc_url());
    let signer =
        InMemorySigner::from_secret_key(args.signer_account.clone(), args.signer_key.clone());

    Ok((
        client.clone(),
        args.markets
            .iter()
            .map(|market| {
                Arc::new(Accumulator::new(
                    client.clone(),
                    signer.clone(),
                    market.clone(),
                    args.timeout,
                ))
            })
            .collect::<Vec<_>>(),
    ))
}
