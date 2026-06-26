use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use near_sdk::AccountId;
use templar_gateway_client::SigningClient;
use templar_gateway_core::GatewayError;
use templar_gateway_types::{common::WriteOperationResult, CryptoHash, OperationStatus};
use tokio::{
    select,
    sync::{mpsc, oneshot, watch, Mutex},
    time::Instant,
};

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

#[derive(thiserror::Error, Debug)]
pub enum UpdateError {
    #[error("failed to fetch oracle update payload: {0}")]
    Fetch(Box<dyn std::error::Error + Send + Sync>),
    #[error("gateway execution failed: {0}")]
    Gateway(#[from] GatewayError),
    #[error("oracle update operation {operation_id} ended with status {status:?}")]
    NotSucceeded {
        operation_id: String,
        status: OperationStatus,
    },
}

/// Interpret a completed gateway write: the latest tx hash on success, an error
/// otherwise. The gateway driver already signed, submitted, and waited for
/// finality, so a reverted operation comes back as `Ok` with a non-`Succeeded`
/// status rather than a transport error.
pub(crate) fn succeeded_tx_hash(
    result: WriteOperationResult,
) -> Result<Option<CryptoHash>, UpdateError> {
    if result.operation.status == OperationStatus::Succeeded {
        Ok(result.operation.latest_tx_hash())
    } else {
        Err(UpdateError::NotSucceeded {
            operation_id: result.operation.id.0,
            status: result.operation.status,
        })
    }
}

#[tracing::instrument(skip_all, name = "oracle_service", fields(oracle_name = S::name()))]
async fn start<S: Spec>(
    mut recv: mpsc::Receiver<Request<S>>,
    spec: Arc<S>,
    gateway: SigningClient,
    kill: watch::Sender<()>,
) {
    let mut client = Client::new(spec, gateway);
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

#[derive(Debug)]
struct PendingBatch<S: Spec> {
    feed_ids: HashSet<S::FeedId>,
    responders: Vec<Responder>,
}

struct Client<S: Spec> {
    last_updated: HashMap<AccountId, HashMap<S::FeedId, Instant>>,
    pending_batches: Mutex<HashMap<AccountId, PendingBatch<S>>>,
    spec: Arc<S>,
    gateway: SigningClient,
}

impl<S: Spec> Client<S> {
    fn new(spec: Arc<S>, gateway: SigningClient) -> Self {
        Self {
            last_updated: HashMap::new(),
            pending_batches: Mutex::new(HashMap::new()),
            spec,
            gateway,
        }
    }

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
        let send_updates_for = price_ids
            .iter()
            .filter(|id| {
                self.last_updated
                    .get(&oracle_id)
                    .and_then(|h| h.get(*id))
                    .is_none_or(|i| i.elapsed() > self.spec.refresh())
            })
            .cloned()
            .collect::<Vec<_>>();

        if send_updates_for.is_empty() {
            return Ok(None);
        }

        tracing::info!(price_ids = ?send_updates_for, "Sending update for prices");

        // Start timing from when we request the prices.
        let now = Instant::now();
        let hash = self
            .spec
            .execute_update(&self.gateway, oracle_id.clone(), &send_updates_for)
            .await?;

        if hash.is_some() {
            self.last_updated
                .entry(oracle_id)
                .or_default()
                .extend(send_updates_for.into_iter().map(|id| (id, now)));
        }

        Ok(hash)
    }
}

#[derive(Debug, Clone)]
pub struct Handle<S: Spec> {
    send: mpsc::Sender<Request<S>>,
}

impl<S: Spec> Handle<S> {
    pub fn new(spec: Arc<S>, gateway: SigningClient, kill: watch::Sender<()>) -> Self {
        let (send, recv) = mpsc::channel(16);
        tokio::spawn(start(recv, spec, gateway, kill));

        Self { send }
    }

    /// # Errors
    ///
    /// - Off-chain payload fetch failure
    /// - Gateway execution failure / non-success operation status
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
