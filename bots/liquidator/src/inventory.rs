// SPDX-License-Identifier: MIT
//! Inventory management for liquidation bot.
//!
//! The `InventoryManager` tracks available balances across all markets and assets,
//! providing a unified view of the bot's capital. This enables inventory-based
//! liquidation where positions are only liquidated when sufficient inventory exists.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    InventoryManager                         │
//! │                                                             │
//! │  Markets Discovery → Assets Extraction → Balance Queries   │
//! │                                                             │
//! │  Cache: HashMap<String, (Asset, InventoryEntry)>           │
//! │         - balance: U128                                     │
//! │         - reserved: U128                                    │
//! │         - last_updated: Instant                             │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```no_run
//! use templar_liquidator::inventory::InventoryManager;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut inventory = InventoryManager::new(client, account_id);
//!
//! // Discover assets from markets
//! inventory.discover_assets(&markets);
//!
//! // Refresh balances
//! inventory.refresh().await?;
//!
//! // Check available balance
//! let available = inventory.get_available_balance(&asset);
//!
//! // Reserve for liquidation
//! inventory.reserve(&asset, amount)?;
//!
//! // After liquidation, release
//! inventory.release(&asset, amount);
//! # Ok(())
//! # }
//! ```

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use near_jsonrpc_client::JsonRpcClient;
use near_sdk::{json_types::U128, AccountId};
use templar_common::asset::{BorrowAsset, FungibleAsset};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::rpc::{view, RpcError};

/// Result type for inventory operations
pub type InventoryResult<T> = Result<T, InventoryError>;

/// Errors that can occur during inventory operations
#[derive(Debug, thiserror::Error)]
pub enum InventoryError {
    #[error("Failed to fetch balance: {0}")]
    FetchBalanceError(#[from] RpcError),

    #[error("Insufficient available balance: required {required}, available {available}")]
    InsufficientBalance { required: u128, available: u128 },

    #[error("Asset not tracked: {0}")]
    AssetNotTracked(String),

    #[error("Invalid asset specification: {0}")]
    InvalidAsset(String),
}

/// Entry tracking a single asset's inventory
#[derive(Debug, Clone)]
struct InventoryEntry {
    /// Total balance
    balance: U128,
    /// Amount reserved for pending liquidations
    reserved: U128,
    /// Last time this balance was updated
    last_updated: Instant,
}

impl InventoryEntry {
    /// Get available (unreserved) balance
    fn available(&self) -> U128 {
        U128(self.balance.0.saturating_sub(self.reserved.0))
    }

    /// Reserve amount for liquidation
    fn reserve(&mut self, amount: U128) -> InventoryResult<()> {
        let available = self.available().0;
        if amount.0 > available {
            return Err(InventoryError::InsufficientBalance {
                required: amount.0,
                available,
            });
        }
        self.reserved.0 = self.reserved.0.saturating_add(amount.0);
        Ok(())
    }

    /// Release reserved amount
    fn release(&mut self, amount: U128) {
        self.reserved.0 = self.reserved.0.saturating_sub(amount.0);
    }

    /// Update balance after refresh
    fn update_balance(&mut self, new_balance: U128) {
        self.balance = new_balance;
        self.last_updated = Instant::now();
    }
}

/// Inventory manager for tracking bot's asset balances
///
/// # Thread Safety
///
/// The `InventoryManager` is wrapped in `Arc<RwLock<_>>` for shared access
/// across async tasks. Multiple readers can access inventory state
/// simultaneously, but writers have exclusive access.
pub struct InventoryManager {
    /// RPC client for balance queries
    client: JsonRpcClient,
    /// Bot's account ID
    account_id: AccountId,
    /// Tracked assets and their balances (keyed by asset string representation)
    inventory: HashMap<String, (FungibleAsset<BorrowAsset>, InventoryEntry)>,
    /// Minimum refresh interval to avoid excessive RPC calls
    min_refresh_interval: Duration,
    /// Last full refresh timestamp
    last_full_refresh: Option<Instant>,
}

impl InventoryManager {
    /// Creates a new inventory manager
    ///
    /// # Arguments
    ///
    /// * `client` - JSON-RPC client for blockchain queries
    /// * `account_id` - Bot's account ID
    pub fn new(client: JsonRpcClient, account_id: AccountId) -> Self {
        Self {
            client,
            account_id,
            inventory: HashMap::new(),
            min_refresh_interval: Duration::from_secs(30),
            last_full_refresh: None,
        }
    }

    /// Sets minimum refresh interval
    #[must_use]
    pub fn with_min_refresh_interval(mut self, interval: Duration) -> Self {
        self.min_refresh_interval = interval;
        self
    }

    /// Discovers assets from market configurations
    ///
    /// Extracts all unique borrow assets across markets and initializes
    /// inventory entries with zero balance.
    ///
    /// # Arguments
    ///
    /// * `market_configs` - Iterator of market configurations
    pub fn discover_assets<'a>(
        &mut self,
        market_configs: impl Iterator<Item = &'a templar_common::market::MarketConfiguration>,
    ) {
        let mut discovered = 0;
        let mut existing = 0;

        for config in market_configs {
            let asset = config.borrow_asset.clone();
            let key = asset.to_string();

            if self.inventory.contains_key(&key) {
                existing += 1;
            } else {
                self.inventory.insert(
                    key.clone(),
                    (
                        asset.clone(),
                        InventoryEntry {
                            balance: U128(0),
                            reserved: U128(0),
                            last_updated: Instant::now(),
                        },
                    ),
                );
                discovered += 1;
                debug!(asset = %asset, "Discovered new asset");
            }
        }

        info!(
            discovered = discovered,
            existing = existing,
            total = self.inventory.len(),
            "Asset discovery complete"
        );
    }

    /// Refreshes all tracked asset balances
    ///
    /// Queries the blockchain for current balances of all tracked assets.
    /// Respects minimum refresh interval to avoid excessive RPC calls.
    ///
    /// # Returns
    ///
    /// # Errors
    ///
    /// Returns an error if balance fetching fails for any asset
    ///
    /// Number of balances successfully refreshed
    pub async fn refresh(&mut self) -> InventoryResult<usize> {
        // Check if we should throttle
        if let Some(last_refresh) = self.last_full_refresh {
            if last_refresh.elapsed() < self.min_refresh_interval {
                debug!(
                    elapsed_ms = last_refresh.elapsed().as_millis(),
                    min_interval_ms = self.min_refresh_interval.as_millis(),
                    "Skipping refresh - too soon since last refresh"
                );
                return Ok(0);
            }
        }

        info!(asset_count = self.inventory.len(), "Refreshing inventory");

        let mut refreshed = 0;
        let mut errors = 0;
        let mut updated_assets = Vec::new();

        // Collect assets to query (clone to avoid borrow issues)
        let assets_to_query: Vec<(String, FungibleAsset<BorrowAsset>)> = self
            .inventory
            .iter()
            .map(|(key, (asset, _))| (key.clone(), asset.clone()))
            .collect();

        for (key, asset) in assets_to_query {
            match self.fetch_balance(&asset).await {
                Ok(balance) => {
                    if let Some((_asset, entry)) = self.inventory.get_mut(&key) {
                        let old_balance = entry.balance.0;
                        entry.update_balance(balance);
                        refreshed += 1;

                        if balance.0 != old_balance {
                            updated_assets.push(format!(
                                "{}({}→{})",
                                asset.to_string().split(':').last().unwrap_or("unknown"),
                                old_balance,
                                balance.0
                            ));
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        asset = %asset,
                        error = %e,
                        "Failed to fetch balance"
                    );
                    errors += 1;
                }
            }
        }

        self.last_full_refresh = Some(Instant::now());

        if updated_assets.is_empty() {
            info!(
                refreshed = refreshed,
                errors = errors,
                "Inventory refresh complete with no balance changes"
            );
        } else {
            info!(
                refreshed = refreshed,
                errors = errors,
                updates = updated_assets.join(", "),
                "Inventory refresh complete with balance changes"
            );
        }

        Ok(refreshed)
    }

    /// Refreshes a single asset's balance
    ///
    /// # Arguments
    ///
    /// * `asset` - Asset to refresh
    ///
    /// # Errors
    ///
    /// Returns an error if balance fetching fails
    pub async fn refresh_asset(
        &mut self,
        asset: &FungibleAsset<BorrowAsset>,
    ) -> InventoryResult<()> {
        let balance = self.fetch_balance(asset).await?;
        let key = asset.to_string();

        if let Some((_asset, entry)) = self.inventory.get_mut(&key) {
            entry.update_balance(balance);
            debug!(
                asset = %asset,
                balance = balance.0,
                available = entry.available().0,
                "Asset balance refreshed"
            );
        } else {
            return Err(InventoryError::AssetNotTracked(asset.to_string()));
        }

        Ok(())
    }

    /// Fetches current balance for an asset from blockchain
    async fn fetch_balance(&self, asset: &FungibleAsset<BorrowAsset>) -> InventoryResult<U128> {
        let balance_action = asset.balance_of_action(&self.account_id);

        let args: serde_json::Value =
            serde_json::from_slice(&balance_action.args).map_err(RpcError::DeserializeError)?;

        let balance = view::<U128>(
            &self.client,
            asset.contract_id().into(),
            &balance_action.method_name,
            args,
        )
        .await?;

        Ok(balance)
    }

    /// Gets available (unreserved) balance for an asset
    ///
    /// # Arguments
    ///
    /// * `asset` - Asset to query
    ///
    /// # Returns
    ///
    /// Available balance, or 0 if asset not tracked
    pub fn get_available_balance(&self, asset: &FungibleAsset<BorrowAsset>) -> U128 {
        let key = asset.to_string();
        self.inventory
            .get(&key)
            .map_or(U128(0), |(_, entry)| entry.available())
    }

    /// Gets total balance (including reserved) for an asset
    pub fn get_total_balance(&self, asset: &FungibleAsset<BorrowAsset>) -> U128 {
        let key = asset.to_string();
        self.inventory
            .get(&key)
            .map_or(U128(0), |(_, entry)| entry.balance)
    }

    /// Gets reserved balance for an asset
    pub fn get_reserved_balance(&self, asset: &FungibleAsset<BorrowAsset>) -> U128 {
        let key = asset.to_string();
        self.inventory
            .get(&key)
            .map_or(U128(0), |(_, entry)| entry.reserved)
    }

    /// Reserves balance for a liquidation
    ///
    /// # Arguments
    ///
    /// * `asset` - Asset to reserve
    /// * `amount` - Amount to reserve
    ///
    /// # Errors
    ///
    /// Returns error if insufficient available balance or asset not tracked
    pub fn reserve(
        &mut self,
        asset: &FungibleAsset<BorrowAsset>,
        amount: U128,
    ) -> InventoryResult<()> {
        let key = asset.to_string();
        let (asset_ref, entry) = self
            .inventory
            .get_mut(&key)
            .ok_or_else(|| InventoryError::AssetNotTracked(asset.to_string()))?;

        entry.reserve(amount)?;

        debug!(
            asset = %asset_ref,
            amount = amount.0,
            available = entry.available().0,
            reserved = entry.reserved.0,
            "Reserved balance"
        );

        Ok(())
    }

    /// Releases reserved balance
    ///
    /// # Arguments
    ///
    /// * `asset` - Asset to release
    /// * `amount` - Amount to release
    pub fn release(&mut self, asset: &FungibleAsset<BorrowAsset>, amount: U128) {
        let key = asset.to_string();
        if let Some((asset_ref, entry)) = self.inventory.get_mut(&key) {
            entry.release(amount);

            debug!(
                asset = %asset_ref,
                amount = amount.0,
                available = entry.available().0,
                reserved = entry.reserved.0,
                "Released balance"
            );
        }
    }

    /// Gets all tracked assets
    pub fn tracked_assets(&self) -> Vec<FungibleAsset<BorrowAsset>> {
        self.inventory
            .values()
            .map(|(asset, _)| asset.clone())
            .collect()
    }

    /// Gets snapshot of current inventory state for logging
    pub fn snapshot(&self) -> InventorySnapshot {
        InventorySnapshot {
            entries: self
                .inventory
                .values()
                .map(|(asset, entry)| InventorySnapshotEntry {
                    asset: asset.to_string(),
                    total: entry.balance.0,
                    available: entry.available().0,
                    reserved: entry.reserved.0,
                    last_updated_ago_ms: u64::try_from(entry.last_updated.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                })
                .collect(),
        }
    }
}

/// Snapshot of inventory state for logging/metrics
#[derive(Debug, Clone, serde::Serialize)]
pub struct InventorySnapshot {
    pub entries: Vec<InventorySnapshotEntry>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InventorySnapshotEntry {
    pub asset: String,
    pub total: u128,
    pub available: u128,
    pub reserved: u128,
    pub last_updated_ago_ms: u64,
}

/// Shared inventory manager for concurrent access
pub type SharedInventory = Arc<RwLock<InventoryManager>>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn create_test_asset() -> FungibleAsset<BorrowAsset> {
        FungibleAsset::from_str("nep141:usdc.near").unwrap()
    }

    #[test]
    fn test_inventory_entry_reserve_release() {
        let mut entry = InventoryEntry {
            balance: U128(1000),
            reserved: U128(0),
            last_updated: Instant::now(),
        };

        // Initial state
        assert_eq!(entry.available().0, 1000);

        // Reserve 300
        entry.reserve(U128(300)).unwrap();
        assert_eq!(entry.available().0, 700);
        assert_eq!(entry.reserved.0, 300);

        // Reserve another 200
        entry.reserve(U128(200)).unwrap();
        assert_eq!(entry.available().0, 500);
        assert_eq!(entry.reserved.0, 500);

        // Try to reserve more than available
        let result = entry.reserve(U128(600));
        assert!(result.is_err());

        // Release 300
        entry.release(U128(300));
        assert_eq!(entry.available().0, 800);
        assert_eq!(entry.reserved.0, 200);

        // Release remaining
        entry.release(U128(200));
        assert_eq!(entry.available().0, 1000);
        assert_eq!(entry.reserved.0, 0);
    }

    #[test]
    fn test_inventory_manager_discovery() {
        use templar_common::market::MarketConfiguration;

        let client = JsonRpcClient::connect("https://rpc.testnet.near.org");
        let account_id = AccountId::from_str("test.near").unwrap();
        let mut inventory = InventoryManager::new(client, account_id);

        // Create mock market configs
        let asset1 = create_test_asset();
        let asset2 = FungibleAsset::from_str("nep141:usdt.near").unwrap();

        let config1 = MarketConfiguration {
            borrow_asset: asset1.clone(),
            ..Default::default()
        };
        let config2 = MarketConfiguration {
            borrow_asset: asset2.clone(),
            ..Default::default()
        };

        // Discover assets
        inventory.discover_assets([&config1, &config2].into_iter());

        assert_eq!(inventory.inventory.len(), 2);
        assert!(inventory.inventory.contains_key(&asset1.to_string()));
        assert!(inventory.inventory.contains_key(&asset2.to_string()));
    }

    #[test]
    fn test_inventory_manager_reserve_release() {
        let client = JsonRpcClient::connect("https://rpc.testnet.near.org");
        let account_id = AccountId::from_str("test.near").unwrap();
        let mut inventory = InventoryManager::new(client, account_id);

        let asset = create_test_asset();
        let key = asset.to_string();

        // Add asset manually
        inventory.inventory.insert(
            key.clone(),
            (
                asset.clone(),
                InventoryEntry {
                    balance: U128(1000),
                    reserved: U128(0),
                    last_updated: Instant::now(),
                },
            ),
        );

        // Check available balance
        assert_eq!(inventory.get_available_balance(&asset).0, 1000);

        // Reserve 300
        inventory.reserve(&asset, U128(300)).unwrap();
        assert_eq!(inventory.get_available_balance(&asset).0, 700);
        assert_eq!(inventory.get_reserved_balance(&asset).0, 300);

        // Release 100
        inventory.release(&asset, U128(100));
        assert_eq!(inventory.get_available_balance(&asset).0, 800);
        assert_eq!(inventory.get_reserved_balance(&asset).0, 200);
    }
}
