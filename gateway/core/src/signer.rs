use std::{iter, sync::Arc};

use futures::future::select_all;
use near_api::{
    types::{
        transaction::{actions::Action, PrepopulateTransaction, SignedTransaction},
        AccountId,
    },
    NetworkConfig, PublicKey, SecretKey, Signer,
};
use templar_gateway_types::ManagedAccountId;
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::{GatewayError, GatewayResult};

/// One pooled access key. NEAR nonces are per access key, so the guard must span
/// nonce allocation through broadcast — hence the signer sits behind the mutex
/// rather than beside it. Assumes no other process holds this same key.
#[derive(Clone)]
struct KeySlot {
    public_key: PublicKey,
    signer: Arc<Mutex<Arc<Signer>>>,
}

impl KeySlot {
    fn new(public_key: PublicKey, signer: Arc<Signer>) -> Self {
        Self {
            public_key,
            signer: Arc::new(Mutex::new(signer)),
        }
    }

    async fn lease(&self, account_id: ManagedAccountId) -> SigningKeyLease {
        SigningKeyLease {
            account_id,
            public_key: self.public_key,
            signer: Arc::clone(&self.signer).lock_owned().await,
        }
    }
}

/// A managed account's access keys, each a serialized nonce lane. Every key is
/// its own single-key [`Signer`], so near-api's access-key rotation never runs
/// and only the lease holder can allocate a nonce on a given key.
///
/// Clones share the same lanes.
#[derive(Clone)]
pub struct PooledSigner {
    account_id: ManagedAccountId,
    /// Split from `others` so a pool always has a lane to hand out.
    primary: KeySlot,
    others: Vec<KeySlot>,
}

impl PooledSigner {
    /// Errors if `secret_keys` is empty or repeats a key. A repeat would build
    /// two lanes over one access key, which is the race this type exists to
    /// prevent.
    pub fn new(
        account_id: impl Into<ManagedAccountId>,
        secret_keys: impl IntoIterator<Item = SecretKey>,
    ) -> GatewayResult<Self> {
        let mut slots: Vec<KeySlot> = Vec::new();

        for secret_key in secret_keys {
            let public_key = secret_key.public_key();
            if slots.iter().any(|slot| slot.public_key == public_key) {
                return Err(GatewayError::InvalidSignerKey(format!(
                    "duplicate access key {public_key}"
                )));
            }
            let signer = Signer::from_secret_key(secret_key)
                .map_err(|error| GatewayError::InvalidSignerKey(error.to_string()))?;
            slots.push(KeySlot::new(public_key, signer));
        }

        let mut slots = slots.into_iter();
        let primary = slots.next().ok_or_else(|| {
            GatewayError::InvalidSignerKey("at least one secret key is required".to_owned())
        })?;

        Ok(Self {
            account_id: account_id.into(),
            primary,
            others: slots.collect(),
        })
    }

    /// Wrap an existing signer's current key as this account's single lane.
    ///
    /// For sharing one [`Signer`] — and so one nonce cache — between pooled and
    /// direct near-api use. The key is taken from the signer rather than named
    /// by the caller, so the lane cannot guard a key the signer does not hold.
    ///
    /// Each call builds a *new* lane, so an account must be wrapped once and the
    /// result cloned; wrapping the same key twice would race it. A lane excludes
    /// other lease holders, not direct use of the same signer.
    pub async fn from_signer(
        account_id: impl Into<ManagedAccountId>,
        signer: Arc<Signer>,
    ) -> GatewayResult<Self> {
        let public_key = signer
            .get_public_key()
            .await
            .map_err(|error| GatewayError::InvalidSignerKey(error.to_string()))?;

        Ok(Self {
            account_id: account_id.into(),
            primary: KeySlot::new(public_key, signer),
            others: Vec::new(),
        })
    }

    #[must_use]
    pub fn account_id(&self) -> &ManagedAccountId {
        &self.account_id
    }

    fn slots(&self) -> impl Iterator<Item = &KeySlot> {
        iter::once(&self.primary).chain(&self.others)
    }

    /// Lease the first idle key, waiting only when every key is already in use.
    pub async fn lease_next(&self) -> SigningKeyLease {
        let (lease, _, _) = select_all(
            self.slots()
                .map(|slot| Box::pin(slot.lease(self.account_id.clone()))),
        )
        .await;
        lease
    }

    /// Lease one specific key — for resubmitting a transaction already signed
    /// with it, whose nonce is bound to that key. `None` when the pool does not
    /// hold the key, and so cannot allocate a nonce on it at all.
    pub async fn lease(&self, public_key: &PublicKey) -> Option<SigningKeyLease> {
        let slot = self.slots().find(|slot| slot.public_key == *public_key)?;
        Some(slot.lease(self.account_id.clone()).await)
    }
}

/// Exclusive right to allocate and broadcast a nonce on one access key. Held
/// from signing through submission; dropping it releases the key.
///
/// The signer stays inside: [`presign`](Self::presign) is the only way to reach
/// it, so a signature can be produced neither on another key nor for another
/// account, and never after the lane has been released.
#[must_use = "dropping a lease releases the key without doing anything with it"]
pub struct SigningKeyLease {
    account_id: ManagedAccountId,
    public_key: PublicKey,
    signer: OwnedMutexGuard<Arc<Signer>>,
}

impl SigningKeyLease {
    #[must_use]
    pub fn account_id(&self) -> &ManagedAccountId {
        &self.account_id
    }

    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        self.public_key
    }

    /// Allocate this key's next nonce and sign under it. The signature is not
    /// yet on the network, so the caller must hold the lease until it is: a
    /// later nonce reaching the chain first invalidates this one.
    pub async fn presign(
        &self,
        network: &NetworkConfig,
        receiver_id: AccountId,
        actions: Vec<Action>,
    ) -> GatewayResult<SignedTransaction> {
        let transaction = PrepopulateTransaction {
            signer_id: self.account_id.0.clone(),
            receiver_id,
            actions,
        };

        let (nonce, block_hash, _) = self
            .signer
            .fetch_tx_nonce(transaction.signer_id.clone(), self.public_key, network)
            .await
            .map_err(|error| GatewayError::NearTransaction(error.to_string()))?;

        self.signer
            .sign(transaction, self.public_key, nonce, block_hash)
            .await
            .map_err(|error| GatewayError::NearTransaction(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use near_api::types::crypto::secret_key::ED25519SecretKey;
    use tokio::time::timeout;

    use super::PooledSigner;
    use near_api::SecretKey;

    fn key(index: u8) -> SecretKey {
        SecretKey::ED25519(ED25519SecretKey::from_secret_key([index + 1; 32]))
    }

    fn pool(secret_keys: impl IntoIterator<Item = SecretKey>) -> PooledSigner {
        PooledSigner::new(
            "pooled.near".parse::<near_api::AccountId>().unwrap(),
            secret_keys,
        )
        .expect("valid pool")
    }

    /// Blind rotation would hand out the busy key and stall behind it while a
    /// free lane sat idle.
    #[tokio::test]
    async fn leases_an_idle_key_rather_than_a_busy_one() {
        let pool = pool([key(0), key(1)]);
        let held = pool.lease(&key(0).public_key()).await.expect("first key");

        let idle = pool.lease_next().await;

        assert_eq!(idle.public_key(), key(1).public_key());
        drop(held);
    }

    #[tokio::test]
    async fn a_key_admits_one_lease_at_a_time() {
        let pool = pool([key(0)]);
        let held = pool.lease_next().await;

        assert!(
            timeout(Duration::from_millis(50), pool.lease_next())
                .await
                .is_err(),
            "the only key must stay held"
        );

        drop(held);
        assert!(timeout(Duration::from_millis(50), pool.lease_next())
            .await
            .is_ok());
    }

    #[test]
    fn rejects_an_empty_pool() {
        assert!(
            PooledSigner::new("pooled.near".parse::<near_api::AccountId>().unwrap(), []).is_err()
        );
    }

    /// Two lanes over one access key would race its nonce — the defect this
    /// type prevents — so a repeated key is a configuration error.
    #[test]
    fn rejects_a_duplicated_key() {
        assert!(PooledSigner::new(
            "pooled.near".parse::<near_api::AccountId>().unwrap(),
            [key(0), key(1), key(0)]
        )
        .is_err());
    }

    #[tokio::test]
    async fn declines_a_key_outside_the_pool() {
        let pool = pool([key(0)]);
        assert!(pool.lease(&key(1).public_key()).await.is_none());
    }

    #[tokio::test]
    async fn leases_carry_the_pool_account() {
        let pool = pool([key(0)]);
        assert_eq!(pool.lease_next().await.account_id(), pool.account_id());
    }
}
