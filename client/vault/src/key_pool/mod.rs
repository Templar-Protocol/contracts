//! Key pool for synchronized multi-key transaction management.
//!
//! This module provides a key pool that allows multiple NEAR access keys to be used
//! for concurrent transaction submission while avoiding nonce conflicts.
//!
//! # Architecture
//!
//! - [`KeySlot`]: Manages a single access key with mutex-protected nonce state
//! - [`KeyPool`]: Manages multiple `KeySlot`s with least-loaded selection
//! - [`KeyPoolClient`]: Drop-in replacement for `Client` with pool-aware transactions
//!
//! # Usage
//!
//! ```ignore
//! let pool_client = KeyPoolClient::new(
//!     "https://rpc.mainnet.near.org".to_string(),
//!     &vault_account,
//!     vec![
//!         KeyCredential { account_id: acc1, secret_key: key1 },
//!         KeyCredential { account_id: acc2, secret_key: key2 },
//!     ],
//!     KeyPoolConfig::default(),
//! )?;
//!
//! // All vault methods available, with automatic key selection and nonce management
//! pool_client.withdraw(&amount, &receiver, &deposit).await?;
//! ```

mod client;
mod health;
mod nonce;
mod pool;
mod slot;

pub use client::{KeyCredential, KeyPoolClient, KeyPoolConfig};
pub use health::{KeyInfo, PoolHealth};
pub use pool::PoolError;
