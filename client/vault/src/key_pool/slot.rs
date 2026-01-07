use std::{
    ops::Deref,
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    time::{Duration, Instant},
};

use anyhow::Result;
use near_account_id::AccountId as NearAccountId;
use near_crypto::{InMemorySigner, PublicKey, SecretKey};
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::hash::CryptoHash;
use templar_common::guard::defer;
use tokio::sync::{Mutex, MutexGuard};
use zeroize::Zeroize;

use super::nonce::fetch_access_key_data;

const DEFAULT_BLOCK_HASH_TTL: Duration = Duration::from_secs(30);

struct ZeroizingSigner(InMemorySigner);

impl Drop for ZeroizingSigner {
    fn drop(&mut self) {
        match &mut self.0.secret_key {
            SecretKey::ED25519(k) => k.0.zeroize(),
            SecretKey::SECP256K1(k) => k.non_secure_erase(),
        }
    }
}

impl Deref for ZeroizingSigner {
    type Target = InMemorySigner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct NonceState {
    local_nonce: Option<u64>,
    block_hash: Option<CryptoHash>,
    block_hash_at: Option<Instant>,
}

impl NonceState {
    fn new() -> Self {
        Self {
            local_nonce: None,
            block_hash: None,
            block_hash_at: None,
        }
    }

    fn needs_refresh(&self, ttl: Duration) -> bool {
        self.block_hash_at
            .map(|t| t.elapsed() > ttl)
            .unwrap_or(true)
            || self.local_nonce.is_none()
    }

    fn invalidate(&mut self) {
        self.local_nonce = None;
        self.block_hash = None;
        self.block_hash_at = None;
    }
}

pub struct KeySlot {
    signer: ZeroizingSigner,
    tx_lock: Mutex<()>,
    nonce_state: Mutex<NonceState>,
    block_hash_ttl: Duration,
    healthy: AtomicBool,
    in_flight: AtomicU32,
    total_txs: AtomicU64,
    total_failures: AtomicU64,
}

impl KeySlot {
    pub fn new(signer: InMemorySigner) -> Self {
        Self::with_config(signer, DEFAULT_BLOCK_HASH_TTL)
    }

    pub fn with_config(signer: InMemorySigner, block_hash_ttl: Duration) -> Self {
        Self {
            signer: ZeroizingSigner(signer),
            tx_lock: Mutex::new(()),
            nonce_state: Mutex::new(NonceState::new()),
            block_hash_ttl,
            healthy: AtomicBool::new(true),
            in_flight: AtomicU32::new(0),
            total_txs: AtomicU64::new(0),
            total_failures: AtomicU64::new(0),
        }
    }

    /// Acquire exclusive access for a transaction. Increments in_flight immediately
    /// (cancellation-safe via defer guard), then waits for tx_lock.
    pub async fn acquire(&self) -> KeySlotGuard<'_> {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        let reservation = defer({
            let in_flight = &self.in_flight;
            move || {
                in_flight.fetch_sub(1, Ordering::Relaxed);
            }
        });

        let _tx_guard = self.tx_lock.lock().await;
        reservation.disarm();

        KeySlotGuard { slot: self, _tx_guard }
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    pub fn in_flight_count(&self) -> u32 {
        self.in_flight.load(Ordering::Relaxed)
    }

    pub fn public_key(&self) -> PublicKey {
        self.signer.public_key()
    }

    pub fn account_id(&self) -> &NearAccountId {
        &self.signer.account_id
    }

    pub fn total_transactions(&self) -> u64 {
        self.total_txs.load(Ordering::Relaxed)
    }

    pub fn total_failures(&self) -> u64 {
        self.total_failures.load(Ordering::Relaxed)
    }

    pub fn mark_unhealthy(&self) {
        self.healthy.store(false, Ordering::Relaxed);
    }

    pub fn mark_healthy(&self) {
        self.healthy.store(true, Ordering::Relaxed);
    }
}

pub struct KeySlotGuard<'a> {
    slot: &'a KeySlot,
    _tx_guard: MutexGuard<'a, ()>,
}

impl Drop for KeySlotGuard<'_> {
    fn drop(&mut self) {
        self.slot.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

impl KeySlotGuard<'_> {
    pub async fn next_nonce(
        &self,
        rpc: &JsonRpcClient,
        timeout: Duration,
    ) -> Result<(u64, CryptoHash)> {
        let cached = {
            let state = self.slot.nonce_state.lock().await;
            if state.needs_refresh(self.slot.block_hash_ttl) {
                None
            } else {
                match (state.local_nonce, state.block_hash) {
                    (Some(nonce), Some(block_hash)) => Some((nonce, block_hash)),
                    _ => None,
                }
            }
        };

        if let Some((nonce, block_hash)) = cached {
            return Ok((nonce, block_hash));
        }

        let (nonce, block_hash) = fetch_access_key_data(
            rpc,
            self.slot.signer.account_id.clone(),
            self.slot.signer.public_key().clone(),
            timeout,
        )
        .await?;

        let mut state = self.slot.nonce_state.lock().await;
        state.local_nonce = Some(nonce);
        state.block_hash = Some(block_hash);
        state.block_hash_at = Some(Instant::now());

        Ok((nonce, block_hash))
    }

    pub async fn advance_nonce(&self) {
        let mut state = self.slot.nonce_state.lock().await;
        if let Some(n) = &mut state.local_nonce {
            *n += 1;
        }
    }

    pub async fn invalidate_nonce(&self) {
        let mut state = self.slot.nonce_state.lock().await;
        state.invalidate();
    }

    pub fn record_success(&self) {
        self.slot.total_txs.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.slot.total_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn mark_unhealthy(&self) {
        self.slot.mark_unhealthy();
    }

    pub fn signer(&self) -> &InMemorySigner {
        &self.slot.signer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_crypto::{KeyType, SecretKey};

    fn test_signer() -> InMemorySigner {
        let account_id: NearAccountId = "test.near".parse().unwrap();
        let secret_key = SecretKey::from_random(KeyType::ED25519);
        InMemorySigner {
            account_id,
            public_key: secret_key.public_key(),
            secret_key,
        }
    }

    #[test]
    fn key_slot_starts_healthy() {
        let slot = KeySlot::new(test_signer());
        assert!(slot.is_healthy());
        assert_eq!(slot.in_flight_count(), 0);
        assert_eq!(slot.total_transactions(), 0);
        assert_eq!(slot.total_failures(), 0);
    }

    #[test]
    fn mark_unhealthy_works() {
        let slot = KeySlot::new(test_signer());
        assert!(slot.is_healthy());
        slot.mark_unhealthy();
        assert!(!slot.is_healthy());
        slot.mark_healthy();
        assert!(slot.is_healthy());
    }

    #[tokio::test]
    async fn acquire_increments_in_flight() {
        let slot = KeySlot::new(test_signer());
        assert_eq!(slot.in_flight_count(), 0);

        let guard = slot.acquire().await;
        assert_eq!(slot.in_flight_count(), 1);

        drop(guard);
        assert_eq!(slot.in_flight_count(), 0);
    }

    #[tokio::test]
    async fn multiple_acquires_serialize() {
        let slot = KeySlot::new(test_signer());

        let guard1 = slot.acquire().await;
        assert_eq!(slot.in_flight_count(), 1);

        let handle = tokio::spawn(async move {});

        drop(guard1);
        handle.await.unwrap();

        let _guard2 = slot.acquire().await;
        assert_eq!(slot.in_flight_count(), 1);
    }
}
