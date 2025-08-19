use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};

use near_crypto::PublicKey;
use near_sdk::{AccountId, NearToken};
use tokio::sync::{mpsc, oneshot};

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

pub struct Cache {
    gas_price: CacheRecord<NearToken>,
    nonce: HashMap<(AccountId, PublicKey), CacheRecord<u64>>,
}

impl Cache {
    pub fn start(near: Near, gas_price_refresh: Duration, nonce_refresh: Duration) -> CacheHandle {
        let (send, mut recv) = mpsc::channel::<CacheRequest>(64);

        tokio::spawn(async move {
            let mut cache = Cache {
                gas_price: CacheRecord::empty(),
                nonce: HashMap::new(),
            };

            let update_gas = || async { near.fetch_gas_price().await };
            let update_nonce = |(account_id, public_key)| {
                || async { near.fetch_nonce(account_id, public_key).await.map(|r| r.0) }
            };

            while let Some(request) = recv.recv().await {
                match request {
                    CacheRequest::Exit(sender) => {
                        tracing::debug!("Received exit signal, exiting...");
                        drop(sender);
                        break;
                    }
                    CacheRequest::GasPrice(sender) => {
                        let fresh = cache.gas_price.fetch(update_gas, gas_price_refresh).await;
                        #[allow(clippy::unwrap_used)]
                        if let Ok(price) = fresh {
                            sender.send(*price).unwrap();
                        } else if let Some(price) = cache.gas_price.stale() {
                            tracing::warn!("Failed to fetch gas price, sending stale value.");
                            sender.send(*price).unwrap();
                        } else {
                            // We should only ever *not* have a stale value on startup, so this should be a "fail-fast" case.
                            tracing::error!("Failed to fetch gas price, no stale value cached.");
                            panic!("Failed to fetch gas price.");
                        }
                    }
                    CacheRequest::Nonce { key, sender } => {
                        let entry = cache
                            .nonce
                            .entry(key.clone())
                            .or_insert_with(CacheRecord::empty);
                        let fresh = entry
                            .fetch_update(update_nonce(key.clone()), nonce_refresh, |n| *n += 1)
                            .await;
                        #[allow(clippy::unwrap_used)]
                        if let Ok(price) = fresh {
                            sender.send(*price).unwrap();
                        } else if let Some(price) = entry.update_stale(|n| *n += 1) {
                            tracing::warn!(
                                "Failed to fetch nonce for {key:?}, sending stale value."
                            );
                            sender.send(*price).unwrap();
                        } else {
                            tracing::error!(
                                "Failed to fetch nonce for {key:?}, no stale value cached."
                            );
                            panic!("Failed to fetch nonce for {key:?}.");
                        }
                    }
                }
            }
        });

        CacheHandle { request: send }
    }
}

enum CacheRequest {
    Exit(oneshot::Sender<()>),
    GasPrice(oneshot::Sender<NearToken>),
    Nonce {
        key: (AccountId, PublicKey),
        sender: oneshot::Sender<u64>,
    },
}

#[derive(Debug)]
pub struct CacheHandle {
    request: mpsc::Sender<CacheRequest>,
}

#[allow(clippy::unwrap_used)]
impl CacheHandle {
    pub async fn gas_price(&self) -> NearToken {
        let (send, recv) = oneshot::channel();
        self.request
            .send(CacheRequest::GasPrice(send))
            .await
            .unwrap();
        recv.await.unwrap()
    }

    pub async fn nonce(&self, account_id: AccountId, public_key: PublicKey) -> u64 {
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

#[allow(clippy::unwrap_used)]
impl Drop for CacheHandle {
    fn drop(&mut self) {
        let r = self.request.clone();

        tokio::spawn(async move {
            let (send, recv) = oneshot::channel();
            r.send(CacheRequest::Exit(send)).await.unwrap();
            recv.await.unwrap_err();
        });
    }
}
