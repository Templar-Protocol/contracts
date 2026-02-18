use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use near_jsonrpc_client::errors::JsonRpcError;
use near_primitives::{
    errors::TxExecutionError,
    hash::CryptoHash,
    views::{FinalExecutionStatus, TxExecutionStatus},
};
use tokio::{
    select,
    sync::{mpsc, oneshot, watch},
    time::Instant,
};

use crate::cache::Cache;

use super::near::Near;

mod spec;
pub use spec::*;

#[derive(Debug)]
pub enum Request<S: Spec> {
    Update {
        price_ids: Box<[S::PriceIdentifier]>,
        send: oneshot::Sender<Result<Option<CryptoHash>, UpdateError>>,
    },
}

#[tracing::instrument(skip_all, name = "oracle_service", fields(oracle_name = S::name()))]
async fn start<S: Spec>(
    mut recv: mpsc::Receiver<Request<S>>,
    spec: Arc<S>,
    near: Near,
    cache: Cache,
    kill: watch::Sender<()>,
) {
    let mut client = Client::new(spec, near, cache);
    let mut on_kill = kill.subscribe();

    loop {
        select! {
            _ = on_kill.changed() => {
                tracing::debug!("Received kill notification.");
                break;
            }
            request = recv.recv() => {
                let Some(request) = request else {
                    tracing::debug!("Sender dropped, exiting.");
                    break;
                };
                match request {
                    Request::Update { price_ids, send } => {
                        #[allow(clippy::unwrap_used, reason = "Sender should not drop")]
                        send.send(client.update(&price_ids).await).unwrap();
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[derive(thiserror::Error, Debug)]
pub enum UpdateError {
    #[error("Failed to construct update transaction: {0}")]
    UpdateActions(Box<dyn std::error::Error + Send + Sync>),
    #[error(transparent)]
    JsonRpc(#[from] JsonRpcError<near_jsonrpc_client::methods::tx::RpcTransactionError>),
    #[error("Unknown RPC error")]
    UnknownRpcError,
    #[error("Transaction execution error: {0}")]
    TransactionExecution(#[from] TxExecutionError),
}

#[derive(Debug)]
struct Client<S: Spec> {
    last_updated: HashMap<S::PriceIdentifier, Instant>,
    spec: Arc<S>,
    near: Near,
    cache: Cache,
}

impl<S: Spec> Client<S> {
    pub fn new(spec: Arc<S>, near: Near, cache: Cache) -> Self {
        Self {
            last_updated: HashMap::new(),
            spec,
            near,
            cache,
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn update(
        &mut self,
        price_ids: &[S::PriceIdentifier],
    ) -> Result<Option<CryptoHash>, UpdateError> {
        let send_updates_for = IntoIterator::into_iter(price_ids)
            .filter(|id| {
                self.last_updated
                    .get(*id)
                    .is_none_or(|i| i.elapsed() > self.spec.refresh())
            })
            .collect::<HashSet<_>>();

        if send_updates_for.is_empty() {
            return Ok(None);
        }

        let send_updates_for: Vec<_> = send_updates_for.into_iter().cloned().collect();

        tracing::info!(price_ids = ?send_updates_for, "Sending update for prices");

        // Start timing from when we request the prices
        let now = Instant::now();
        let actions = self
            .spec
            .update_actions(&send_updates_for)
            .await
            .map_err(|e| UpdateError::UpdateActions(Box::new(e)))?;
        tracing::debug!(?actions, "Update actions");
        let signed_transaction = self
            .near
            .sign_transaction(&self.cache, self.spec.oracle_id().to_owned(), actions)
            .await;
        tracing::debug!(?signed_transaction, "Signed oracle update transaction");

        let transaction_hash = signed_transaction.get_hash();

        let transaction_result = self
            .near
            .send_transaction(signed_transaction, TxExecutionStatus::Final)
            .await?;
        tracing::debug!(?transaction_result, "Oracle update transaction sent");

        if let Some(o) = transaction_result.final_execution_outcome {
            match o.into_outcome().status {
                FinalExecutionStatus::NotStarted | FinalExecutionStatus::Started => {
                    // Should never happen because we waited until TxExecutionStatus::Final
                    tracing::warn!("Unexpected transaction execution status retrieved from RPC");
                    Err(UpdateError::UnknownRpcError)
                }
                FinalExecutionStatus::Failure(error) => {
                    tracing::error!(?error, "Oracle update transaction failed");
                    Err(error.into())
                }
                FinalExecutionStatus::SuccessValue(..) => {
                    tracing::debug!("Oracle update succeeded");

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
}

#[derive(Debug, Clone)]
pub struct Handle<S: Spec> {
    send: mpsc::Sender<Request<S>>,
}

impl<S: Spec> Handle<S> {
    pub fn new(spec: Arc<S>, near: Near, cache: Cache, kill: watch::Sender<()>) -> Self {
        let (send, recv) = mpsc::channel(16);
        tokio::spawn(start(recv, spec, near, cache, kill));

        Self { send }
    }

    /// # Errors
    ///
    /// - Network error [`reqwest::Error`]
    /// - JSON RPC error
    /// - Unexpected/inconsistent RPC behavior
    /// - Transaction failure
    #[allow(clippy::unwrap_used)]
    #[tracing::instrument(skip(self))]
    pub async fn update(
        &self,
        price_ids: Box<[S::PriceIdentifier]>,
    ) -> Result<Option<CryptoHash>, UpdateError> {
        let (send, recv) = oneshot::channel();
        self.send
            .send(Request::Update { price_ids, send })
            .await
            .unwrap();
        recv.await.unwrap()
    }
}
