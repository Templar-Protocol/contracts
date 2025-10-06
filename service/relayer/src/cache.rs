use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime},
};

use near_crypto::PublicKey;
use near_jsonrpc_client::{
    errors::JsonRpcError,
    methods::{query::RpcQueryError, EXPERIMENTAL_protocol_config::RpcProtocolConfigResponse},
};
use near_primitives::hash::CryptoHash;
use near_sdk::{AccountId, NearToken};
use tokio::{
    select,
    sync::{mpsc, oneshot, watch, RwLock},
};

use crate::client::near::Near;

#[derive(Debug)]
struct CacheRecord<T> {
    value: Option<T>,
    updated_at: SystemTime,
}

impl<T> CacheRecord<T> {
    pub fn empty() -> Self {
        Self {
            value: None,
            updated_at: SystemTime::now(),
        }
    }

    pub fn stale(&self) -> Option<&T> {
        self.value.as_ref()
    }

    pub fn update_stale(&mut self, update: impl FnOnce(&mut T)) -> Option<&T> {
        self.value.as_mut().map(|value| {
            update(value);
            &*value
        })
    }

    pub async fn fetch<E>(
        &mut self,
        fetch_fresh: impl AsyncFnOnce() -> Result<T, E>,
        maximum_age: Duration,
    ) -> Result<&T, E> {
        self.fetch_update(fetch_fresh, maximum_age, |_| {}).await
    }

    pub async fn fetch_update<E>(
        &mut self,
        fetch_fresh: impl AsyncFnOnce() -> Result<T, E>,
        maximum_age: Duration,
        and_transform: impl FnOnce(&mut T),
    ) -> Result<&T, E> {
        if self.value.is_some()
            && self
                .updated_at
                .elapsed()
                .is_ok_and(|elapsed| elapsed <= maximum_age)
        {
            #[allow(clippy::unwrap_used, reason = "Guaranteed by .is_some() call")]
            let v = self.value.as_mut().unwrap();
            and_transform(v);
            return Ok(v);
        }

        let mut v = fetch_fresh().await?;
        self.updated_at = SystemTime::now();
        and_transform(&mut v);
        let r = self.value.insert(v);
        Ok(r)
    }
}

#[derive(Debug)]
pub struct Cache {
    request: mpsc::Sender<CacheRequest>,
}

async fn start(
    mut recv: mpsc::Receiver<CacheRequest>,
    near: Near,
    cache_config: crate::app::args::Cache,
    kill: watch::Sender<()>,
) {
    let mut config = CacheRecord::empty();
    let mut gas_price = CacheRecord::empty();
    let mut nonce = HashMap::<(AccountId, PublicKey), CacheRecord<u64>>::new();
    let block_hash = Arc::new(RwLock::new(CryptoHash::new()));

    let update_config = || async { near.fetch_protocol_configuration().await.map(Arc::new) };
    let update_gas = || async { near.fetch_gas_price().await };
    let update_nonce = |(account_id, public_key)| {
        || async {
            let (nonce, hash) = near.fetch_nonce(account_id, public_key).await?;
            *block_hash.write().await = hash;
            Ok::<_, JsonRpcError<RpcQueryError>>(nonce + 1)
        }
    };

    let exec_kill = |msg: &str| {
        tracing::error!("{msg}");
        #[allow(clippy::unwrap_used, reason = "We're panicking here anyways")]
        kill.send(()).unwrap();
        panic!("{msg}");
    };

    let mut on_kill = kill.subscribe();

    loop {
        select! {
            _ = on_kill.changed() => {
                tracing::debug!("Received kill notification.");
                break;
            }
            request = recv.recv() => {
                let Some(request) = request else {
                    tracing::debug!("Cache sender dropped, exiting.");
                    break;
                };
                match request {
                    CacheRequest::ProtocolConfig(sender) => {
                        let fresh = config.fetch(update_config, cache_config.protocol_config_refresh).await;
                        #[allow(clippy::unwrap_used)]
                        if let Ok(value) = fresh {
                            sender.send(Arc::clone(value)).unwrap();
                        } else if let Some(value) = config.stale() {
                            tracing::warn!("Failed to fetch protocol config, sending stale value.");
                            sender.send(Arc::clone(value)).unwrap();
                        } else {
                            exec_kill("Failed to fetch protocol config, no stale value cached.");
                        }
                    }
                    CacheRequest::GasPrice(sender) => {
                        let fresh = gas_price.fetch(update_gas, cache_config.gas_price_refresh).await;
                        #[allow(clippy::unwrap_used)]
                        if let Ok(price) = fresh {
                            sender.send(*price).unwrap();
                        } else if let Some(price) = gas_price.stale() {
                            tracing::warn!("Failed to fetch gas price, sending stale value.");
                            sender.send(*price).unwrap();
                        } else {
                            // We should only ever *not* have a stale value on startup, so this should be a "fail-fast" case.
                            exec_kill("Failed to fetch gas price, no stale value cached.");
                        }
                    }
                    CacheRequest::Nonce { key, sender } => {
                        let entry = nonce.entry(key.clone()).or_insert_with(CacheRecord::empty);
                        let fresh = entry
                            .fetch_update(update_nonce(key.clone()), cache_config.nonce_refresh, |n| *n += 1)
                            .await;
                        #[allow(clippy::unwrap_used)]
                        if let Ok(nonce) = fresh {
                            sender.send((*nonce, *block_hash.read().await)).unwrap();
                        } else if let Some(nonce) = entry.update_stale(|n| *n += 1) {
                            tracing::warn!(
                                "Failed to fetch nonce for {key:?}, sending stale value."
                            );
                            sender.send((*nonce, *block_hash.read().await)).unwrap();
                        } else {
                            exec_kill(&format!(
                                "Failed to fetch nonce for {key:?}, no stale value cached."
                            ));
                        }
                    }
                }
            }
        }
    }
}

impl Cache {
    pub fn new(near: Near, config: crate::app::args::Cache, kill: watch::Sender<()>) -> Self {
        let (send, recv) = mpsc::channel::<CacheRequest>(64);

        tokio::spawn(start(recv, near, config, kill));

        Self { request: send }
    }
}

enum CacheRequest {
    GasPrice(oneshot::Sender<NearToken>),
    Nonce {
        key: (AccountId, PublicKey),
        sender: oneshot::Sender<(u64, CryptoHash)>,
    },
    ProtocolConfig(oneshot::Sender<Arc<RpcProtocolConfigResponse>>),
}

#[allow(clippy::unwrap_used)]
impl Cache {
    pub async fn gas_price(&self) -> NearToken {
        let (send, recv) = oneshot::channel();
        self.request
            .send(CacheRequest::GasPrice(send))
            .await
            .unwrap();
        recv.await.unwrap()
    }

    pub async fn nonce(&self, account_id: AccountId, public_key: PublicKey) -> (u64, CryptoHash) {
        let (send, recv) = oneshot::channel();
        self.request
            .send(CacheRequest::Nonce {
                key: (account_id, public_key),
                sender: send,
            })
            .await
            .unwrap();
        recv.await.unwrap()
    }
}
