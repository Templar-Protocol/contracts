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
use near_sdk::AccountId;
use tokio::{
    select,
    sync::{mpsc, oneshot, watch, Mutex},
    time::Instant,
};

use crate::cache::Cache;

use super::near::Near;

mod spec;
pub use spec::*;

type Responder = oneshot::Sender<Result<Option<CryptoHash>, Arc<UpdateError>>>;

#[derive(Debug)]
pub enum Request<S: Spec> {
    Update {
        oracle_id: AccountId,
        feed_ids: Box<[S::FeedId]>,
        send: Responder,
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

    let mut batch_timer = tokio::time::interval(Duration::from_millis(100));

    loop {
        select! {
            _ = on_kill.changed() => {
                tracing::debug!("Received kill notification.");
                break;
            }
            _ = batch_timer.tick() => {
                let batches = {
                    let mut batches = client.pending_batches.lock().await;
                    std::mem::take(&mut *batches)
                };

                for (oracle_id, batch) in batches {
                    let feed_ids = batch.feed_ids.iter().cloned().collect::<Vec<_>>();
                    let result = client.update(oracle_id.clone(), &feed_ids).await.map_err(Arc::new);
                    if let Err(ref e) = result {
                        tracing::error!(?oracle_id, error = ?e, "Failed to update oracle feed");
                    }
                    for responder in batch.responders {
                        if responder.send(result.clone()).is_err() {
                            tracing::error!("Failed to send update result to requester: sender dropped");
                        }
                    }
                }
            }
            request = recv.recv() => {
                let Some(request) = request else {
                    tracing::debug!("Sender dropped, exiting.");
                    break;
                };
                match request {
                    Request::Update { oracle_id, feed_ids, send } => {
                        client.add_to_batch(oracle_id, &feed_ids, send).await;
                    }
                }
            }
        }
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
struct PendingBatch<S: Spec> {
    feed_ids: HashSet<S::FeedId>,
    responders: Vec<Responder>,
}

#[derive(Debug)]
struct Client<S: Spec> {
    last_updated: HashMap<AccountId, HashMap<S::FeedId, Instant>>,
    pending_batches: Mutex<HashMap<AccountId, PendingBatch<S>>>,
    spec: Arc<S>,
    near: Near,
    cache: Cache,
}

impl<S: Spec> Client<S> {
    fn new(spec: Arc<S>, near: Near, cache: Cache) -> Self {
        Self {
            last_updated: HashMap::new(),
            pending_batches: Mutex::new(HashMap::new()),
            spec,
            near,
            cache,
        }
    }

    #[tracing::instrument(skip(self))]
    async fn add_to_batch(
        &self,
        oracle_id: AccountId,
        price_ids: &[S::FeedId],
        responder: Responder,
    ) {
        let mut batches = self.pending_batches.lock().await;
        let batch = batches
            .entry(oracle_id.clone())
            .or_insert_with(|| PendingBatch {
                feed_ids: HashSet::new(),
                responders: vec![],
            });
        batch.feed_ids.extend(price_ids.iter().cloned());
        batch.responders.push(responder);
    }

    #[tracing::instrument(skip(self))]
    async fn update(
        &mut self,
        oracle_id: AccountId,
        price_ids: &[S::FeedId],
    ) -> Result<Option<CryptoHash>, UpdateError> {
        let send_updates_for = IntoIterator::into_iter(price_ids)
            .filter(|id| {
                self.last_updated
                    .get(&oracle_id)
                    .and_then(|h| h.get(*id))
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
        if actions.is_empty() {
            tracing::debug!("No actions to send for this update");
            return Ok(None);
        }
        tracing::debug!(?actions, "Update actions");
        let signed_transaction = self
            .near
            .sign_transaction(&self.cache, oracle_id.clone(), actions)
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
                        .entry(oracle_id)
                        .or_default()
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
        oracle_id: AccountId,
        feed_ids: Box<[S::FeedId]>,
    ) -> Result<Option<CryptoHash>, Arc<UpdateError>> {
        let (send, recv) = oneshot::channel();
        self.send
            .send(Request::Update {
                oracle_id,
                feed_ids,
                send,
            })
            .await
            .unwrap();
        recv.await.unwrap()
    }
}
