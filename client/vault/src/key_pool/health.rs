//! Health and observability types for key pool monitoring.

use crate::AccountId;

use super::pool::KeyPool;

/// Information about a single key in the pool.
#[derive(uniffi::Record, Debug, Clone)]
pub struct KeyInfo {
    /// The public key (base58 encoded).
    pub public_key: String,

    /// The account ID that owns this key.
    pub account_id: AccountId,

    /// Number of transactions currently in-flight for this key.
    pub in_flight: u32,

    /// Whether this key is healthy and can be used.
    pub is_healthy: bool,

    /// Total number of successful transactions submitted with this key.
    pub total_transactions: u64,

    /// Total number of failed transactions for this key.
    pub total_failures: u64,
}

/// Overall health status of the key pool.
#[derive(uniffi::Record, Debug, Clone)]
pub struct PoolHealth {
    /// Total number of keys in the pool.
    pub total_keys: u32,

    /// Number of keys currently marked healthy.
    pub healthy_keys: u32,

    /// Total transactions in-flight across all keys.
    pub total_in_flight: u32,

    /// Per-key health information.
    pub keys: Vec<KeyInfo>,
}

impl PoolHealth {
    /// Build health report from a key pool.
    pub fn from_pool(pool: &KeyPool) -> Self {
        let keys: Vec<KeyInfo> = pool
            .slots()
            .iter()
            .map(|slot| KeyInfo {
                public_key: slot.public_key().to_string(),
                account_id: AccountId(slot.account_id().to_string()),
                in_flight: slot.in_flight_count(),
                is_healthy: slot.is_healthy(),
                total_transactions: slot.total_transactions(),
                total_failures: slot.total_failures(),
            })
            .collect();

        Self {
            total_keys: pool.len() as u32,
            healthy_keys: pool.healthy_count() as u32,
            total_in_flight: pool.total_in_flight(),
            keys,
        }
    }
}
