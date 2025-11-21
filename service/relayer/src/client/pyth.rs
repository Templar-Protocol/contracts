use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use near_jsonrpc_client::errors::JsonRpcError;
use near_primitives::{
    errors::TxExecutionError,
    hash::CryptoHash,
    views::{FinalExecutionStatus, TxExecutionStatus},
};
use near_sdk::serde::Deserialize;
use templar_common::oracle::pyth::PriceIdentifier;
use tokio::{
    select,
    sync::{mpsc, oneshot, watch},
    time::Instant,
};

use crate::{app::args, cache::Cache};

use super::near::Near;

#[derive(Debug)]
pub enum PythRequest {
    Update {
        price_ids: Box<[PriceIdentifier]>,
        send: oneshot::Sender<Result<Option<CryptoHash>, UpdateError>>,
    },
}

#[tracing::instrument(skip_all, name = "pyth_service")]
async fn start(
    mut recv: mpsc::Receiver<PythRequest>,
    args: args::Pyth,
    near: Near,
    cache: Cache,
    kill: watch::Sender<()>,
) {
    let mut pyth = PythClient::new(args, near, cache);
    let mut on_kill = kill.subscribe();

    loop {
        select! {
            _ = on_kill.changed() => {
                tracing::debug!("Received kill notification.");
                break;
            }
            request = recv.recv() => {
                let Some(request) = request else {
                    tracing::debug!("Pyth sender dropped, exiting.");
                    break;
                };
                tracing::debug!("Handling request: {request:?}");
                match request {
                    PythRequest::Update { price_ids, send } => {
                        #[allow(clippy::unwrap_used, reason = "Sender should not drop")]
                        send.send(pyth.update(&price_ids).await).unwrap();
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[derive(thiserror::Error, Debug)]
pub enum UpdateError {
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error(transparent)]
    JsonRpc(#[from] JsonRpcError<near_jsonrpc_client::methods::tx::RpcTransactionError>),
    #[error("Unknown RPC error")]
    UnknownRpcError,
    #[error("Transaction execution error: {0}")]
    TransactionExecution(#[from] TxExecutionError),
}

#[derive(Debug)]
struct PythClient {
    http: reqwest::Client,
    last_updated: HashMap<PriceIdentifier, Instant>,
    args: args::Pyth,
    near: Near,
    cache: Cache,
}

impl PythClient {
    pub fn new(args: args::Pyth, near: Near, cache: Cache) -> Self {
        Self {
            http: reqwest::Client::new(),
            last_updated: HashMap::new(),
            args,
            near,
            cache,
        }
    }

    pub async fn update(
        &mut self,
        price_ids: &[PriceIdentifier],
    ) -> Result<Option<CryptoHash>, UpdateError> {
        let send_updates_for = IntoIterator::into_iter(price_ids)
            .filter(|id| {
                self.last_updated
                    .get(id)
                    .is_none_or(|i| i.elapsed() > self.args.refresh)
            })
            .collect::<HashSet<_>>();

        if send_updates_for.is_empty() {
            return Ok(None);
        }

        let send_updates_for: Vec<_> = send_updates_for.into_iter().copied().collect();

        tracing::info!(price_ids = ?send_updates_for, "Sending update for Pyth prices");

        // Start timing from when we request the prices
        let now = Instant::now();
        let vaa = self.get_latest_price_updates_vaa(&send_updates_for).await?;
        tracing::debug!(vaa = hex::encode(&vaa), "Retrieved VAA");
        let signed_transaction = self
            .near
            .construct_pyth_update_transaction(
                &self.cache,
                self.args.oracle_id.clone(),
                vaa,
                self.args.update_gas,
                self.args.update_deposit,
            )
            .await;
        tracing::debug!(?signed_transaction, "Signed Pyth update transaction.");

        let transaction_hash = signed_transaction.get_hash();

        let transaction_result = self
            .near
            .send_transaction(signed_transaction, TxExecutionStatus::Final)
            .await?;
        tracing::debug!(?transaction_result, "Pyth update transaction sent");

        if let Some(o) = transaction_result.final_execution_outcome {
            match o.into_outcome().status {
                FinalExecutionStatus::NotStarted | FinalExecutionStatus::Started => {
                    // Should never happen because we waited until TxExecutionStatus::Final
                    tracing::warn!("Unexpected transaction execution status retrieved from RPC");
                    Err(UpdateError::UnknownRpcError)
                }
                FinalExecutionStatus::Failure(error) => {
                    tracing::error!(?error, "Pyth update transaction failed");
                    Err(error.into())
                }
                FinalExecutionStatus::SuccessValue(..) => {
                    tracing::debug!("Pyth update succeeded");

                    self.last_updated
                        .extend(send_updates_for.into_iter().map(|id| (id, now)));

                    Ok(Some(transaction_hash))
                }
            }
        } else {
            tracing::warn!("Unable to retrieve final execution outcome from RPC");
            Err(UpdateError::UnknownRpcError)
        }
    }

    /// Fetch just the update payload for a set of price IDs.
    ///
    /// # Errors
    ///
    /// - [`reqwest::Error`]
    /// - Response deserialization.
    #[tracing::instrument(skip(self))]
    pub async fn get_latest_price_updates_vaa(
        &self,
        price_ids: &[PriceIdentifier],
    ) -> Result<Vec<u8>, reqwest::Error> {
        #[derive(Deserialize)]
        #[serde(crate = "near_sdk::serde")]
        struct ResponseBody {
            binary: Binary,
        }

        #[derive(Deserialize)]
        #[serde(crate = "near_sdk::serde")]
        struct Binary {
            data: [Data; 1],
        }

        #[derive(Deserialize)]
        #[serde(crate = "near_sdk::serde")]
        struct Data(#[serde(deserialize_with = "hex::deserialize")] Vec<u8>);

        let mut request = self
            .http
            .get(format!("{}/v2/updates/price/latest", self.args.hermes_url));

        for id in price_ids {
            request = request.query(&[("ids[]", id)]);
        }

        let response = request.send().await?.error_for_status()?;

        let body = response.json::<ResponseBody>().await?;
        let [vaa] = body.binary.data;
        Ok(vaa.0)
    }
}

#[derive(Debug, Clone)]
pub struct Pyth {
    send: mpsc::Sender<PythRequest>,
}

impl Pyth {
    pub fn new(args: args::Pyth, near: Near, cache: Cache, kill: watch::Sender<()>) -> Self {
        let (send, recv) = mpsc::channel(16);
        tokio::spawn(start(recv, args, near, cache, kill));

        Self { send }
    }

    /// # Errors
    ///
    /// - Network error [`reqwest::Error`]
    /// - JSON RPC error
    /// - Unexpected/inconsistent RPC behavior
    /// - Transaction failure
    #[allow(clippy::unwrap_used)]
    pub async fn update(
        &self,
        price_ids: Box<[PriceIdentifier]>,
    ) -> Result<Option<CryptoHash>, UpdateError> {
        let (send, recv) = oneshot::channel();
        self.send
            .send(PythRequest::Update { price_ids, send })
            .await
            .unwrap();
        recv.await.unwrap()
    }
}
