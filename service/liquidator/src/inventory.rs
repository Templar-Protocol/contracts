//! Inventory management for liquidation bot.
//!
//! The `InventoryManager` tracks available balances across all markets and assets,
//! providing a unified view of the bot's capital. This enables inventory-based
//! liquidation where positions are only liquidated when sufficient inventory exists.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use near_sdk::{json_types::U128, serde::Serialize, AccountId};
use templar_common::asset::{
    BorrowAsset, BorrowAssetAmount, CollateralAsset, CollateralAssetAmount, FungibleAsset,
    FungibleAssetAmount,
};
use templar_gateway_client::SigningClient;
use templar_gateway_methods_spec::token::{self, TokenReference};
use tokio::sync::RwLock;

use crate::rpc::RpcError;

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
struct InventoryEntry<T: templar_common::asset::AssetClass> {
    /// Total balance
    balance: FungibleAssetAmount<T>,
    /// Amount reserved for pending liquidations
    reserved: FungibleAssetAmount<T>,
    /// Last time this balance was updated
    last_updated: Instant,
}

impl<T: templar_common::asset::AssetClass> InventoryEntry<T> {
    /// Get available (unreserved) balance
    fn available(&self) -> FungibleAssetAmount<T> {
        FungibleAssetAmount::from(
            u128::from(self.balance).saturating_sub(u128::from(self.reserved)),
        )
    }

    /// Reserve amount for liquidation
    fn reserve(&mut self, amount: FungibleAssetAmount<T>) -> InventoryResult<()> {
        let available = u128::from(self.available());
        let amount_u128 = u128::from(amount);
        if amount_u128 > available {
            return Err(InventoryError::InsufficientBalance {
                required: amount_u128,
                available,
            });
        }
        self.reserved =
            FungibleAssetAmount::from(u128::from(self.reserved).saturating_add(amount_u128));
        Ok(())
    }

    /// Release reserved amount
    fn release(&mut self, amount: FungibleAssetAmount<T>) {
        self.reserved =
            FungibleAssetAmount::from(u128::from(self.reserved).saturating_sub(u128::from(amount)));
    }

    /// Update balance after refresh
    fn update_balance(&mut self, new_balance: FungibleAssetAmount<T>) {
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
    /// Gateway client for balance queries
    client: SigningClient,
    /// Bot's account ID
    account_id: AccountId,
    /// Tracked borrow assets and their balances
    inventory: HashMap<FungibleAsset<BorrowAsset>, InventoryEntry<BorrowAsset>>,
    /// Tracked collateral assets (received from liquidations)
    collateral_inventory: HashMap<FungibleAsset<CollateralAsset>, InventoryEntry<CollateralAsset>>,
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
    /// * `client` - Gateway client for blockchain queries
    /// * `account_id` - Bot's account ID
    pub fn new(client: SigningClient, account_id: AccountId) -> Self {
        Self {
            client,
            account_id,
            inventory: HashMap::new(),
            collateral_inventory: HashMap::new(),
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

            match self.inventory.entry(asset.clone()) {
                std::collections::hash_map::Entry::Occupied(_) => {
                    existing += 1;
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(InventoryEntry {
                        balance: BorrowAssetAmount::from(0),
                        reserved: BorrowAssetAmount::from(0),
                        last_updated: Instant::now(),
                    });
                    discovered += 1;
                }
            }
        }

        tracing::info!(
            discovered = discovered,
            existing = existing,
            total = self.inventory.len(),
            "Discovered borrow assets from market configurations"
        );
    }

    /// Discovers collateral assets from market configurations
    pub fn discover_collateral_assets<'a>(
        &mut self,
        market_configs: impl Iterator<Item = &'a templar_common::market::MarketConfiguration>,
    ) {
        let mut discovered = 0;
        let mut existing = 0;

        for config in market_configs {
            let asset = config.collateral_asset.clone();

            match self.collateral_inventory.entry(asset.clone()) {
                std::collections::hash_map::Entry::Occupied(_) => {
                    existing += 1;
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(InventoryEntry {
                        balance: CollateralAssetAmount::from(0),
                        reserved: CollateralAssetAmount::from(0),
                        last_updated: Instant::now(),
                    });
                    discovered += 1;
                }
            }
        }

        tracing::info!(
            discovered = discovered,
            existing = existing,
            total = self.collateral_inventory.len(),
            "Discovered collateral assets from market configurations"
        );
    }

    /// Refreshes all tracked asset balances
    ///
    /// # Errors
    ///
    /// Returns an error if the RPC call to fetch balances fails.
    pub async fn refresh(&mut self) -> InventoryResult<usize> {
        // Check if we should throttle
        if let Some(last_refresh) = self.last_full_refresh {
            if last_refresh.elapsed() < self.min_refresh_interval {
                tracing::debug!(
                    elapsed_ms = last_refresh.elapsed().as_millis(),
                    min_interval_ms = self.min_refresh_interval.as_millis(),
                    "Skipping refresh - too soon since last refresh"
                );
                return Ok(0);
            }
        }

        tracing::info!(asset_count = self.inventory.len(), "Refreshing inventory");

        let mut refreshed = 0;
        let mut errors = 0;
        let mut updated_assets = Vec::new();

        // Collect assets to query (clone to avoid borrow issues)
        let assets_to_query: Vec<FungibleAsset<BorrowAsset>> =
            self.inventory.keys().cloned().collect();

        for asset in assets_to_query {
            match self.fetch_balance(&asset).await {
                Ok(balance) => {
                    if let Some(entry) = self.inventory.get_mut(&asset) {
                        let old_balance = u128::from(entry.balance);
                        entry.update_balance(BorrowAssetAmount::from(balance.0));
                        refreshed += 1;

                        if balance.0 != old_balance {
                            updated_assets.push(format!(
                                "{}({}→{})",
                                asset
                                    .to_string()
                                    .split(':')
                                    .next_back()
                                    .unwrap_or("unknown"),
                                old_balance,
                                balance.0
                            ));
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        asset = %asset,
                        error = %e,
                        "Failed to fetch balance"
                    );
                    errors += 1;
                }
            }
        }

        self.last_full_refresh = Some(Instant::now());

        // Show all borrow assets with non-zero balance (full asset IDs)
        let available_assets: Vec<String> = self
            .inventory
            .iter()
            .filter_map(|(asset, entry)| {
                if u128::from(entry.balance) == 0 {
                    return None;
                }
                Some(asset.to_string())
            })
            .collect();

        if available_assets.is_empty() {
            tracing::info!(
                refreshed = refreshed,
                errors = errors,
                "Borrow asset inventory refresh complete - no assets with balance"
            );
        } else {
            tracing::info!(
                refreshed = refreshed,
                errors = errors,
                available_borrow_assets = available_assets.join(", "),
                "Borrow asset inventory refresh complete"
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

        if let Some(entry) = self.inventory.get_mut(asset) {
            entry.update_balance(BorrowAssetAmount::from(balance.0));
        } else {
            return Err(InventoryError::AssetNotTracked(asset.to_string()));
        }

        Ok(())
    }

    /// Fetches current balance for an asset from blockchain
    async fn fetch_balance(&self, asset: &FungibleAsset<BorrowAsset>) -> InventoryResult<U128> {
        let result = self
            .client
            .read(token::GetBalanceOf {
                token: TokenReference::from(asset),
                account_id: self.account_id.clone(),
            })
            .await
            .map_err(|e| InventoryError::FetchBalanceError(e.into()))?;

        Ok(U128(result.balance.0))
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
        U128::from(u128::from(
            self.inventory
                .get(asset)
                .map_or(BorrowAssetAmount::from(0), |entry| entry.available()),
        ))
    }

    /// Gets total balance (including reserved) for an asset
    pub fn get_total_balance(&self, asset: &FungibleAsset<BorrowAsset>) -> U128 {
        U128::from(u128::from(
            self.inventory
                .get(asset)
                .map_or(BorrowAssetAmount::from(0), |entry| entry.balance),
        ))
    }

    /// Gets reserved balance for an asset
    pub fn get_reserved_balance(&self, asset: &FungibleAsset<BorrowAsset>) -> U128 {
        U128::from(u128::from(
            self.inventory
                .get(asset)
                .map_or(BorrowAssetAmount::from(0), |entry| entry.reserved),
        ))
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
        amount: BorrowAssetAmount,
    ) -> InventoryResult<()> {
        let entry = self
            .inventory
            .get_mut(asset)
            .ok_or_else(|| InventoryError::AssetNotTracked(asset.to_string()))?;

        entry.reserve(amount)?;

        Ok(())
    }

    /// Releases reserved balance
    ///
    /// # Arguments
    ///
    /// * `asset` - Asset to release
    /// * `amount` - Amount to release
    pub fn release(&mut self, asset: &FungibleAsset<BorrowAsset>, amount: BorrowAssetAmount) {
        if let Some(entry) = self.inventory.get_mut(asset) {
            entry.release(amount);
        }
    }

    /// Gets all tracked assets
    pub fn tracked_assets(&self) -> Vec<FungibleAsset<BorrowAsset>> {
        self.inventory.keys().cloned().collect()
    }

    /// Gets snapshot of current inventory state for logging
    pub fn snapshot(&self) -> InventorySnapshot {
        InventorySnapshot {
            entries: self
                .inventory
                .iter()
                .map(|(asset, entry)| InventorySnapshotEntry {
                    asset: asset.to_string(),
                    total: u128::from(entry.balance),
                    available: u128::from(entry.available()),
                    reserved: u128::from(entry.reserved),
                    last_updated_ago_ms: u64::try_from(entry.last_updated.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                })
                .collect(),
        }
    }

    /// Refreshes all collateral asset balances
    ///
    /// Similar to `refresh()` but for collateral assets received from liquidations.
    /// Returns a map of non-zero collateral balances.
    ///
    /// # Returns
    ///
    /// `HashMap` of asset name to balance for assets with non-zero balance
    ///
    /// # Errors
    ///
    /// Returns error if fetching fails
    pub async fn refresh_collateral(&mut self) -> InventoryResult<HashMap<String, U128>> {
        tracing::info!(
            collateral_asset_count = self.collateral_inventory.len(),
            "Refreshing collateral inventory"
        );

        let mut non_zero_balances = HashMap::new();
        let mut refreshed = 0;
        let mut errors = 0;

        // Collect assets to query (clone to avoid borrow issues)
        let assets_to_query: Vec<FungibleAsset<CollateralAsset>> =
            self.collateral_inventory.keys().cloned().collect();

        for asset in assets_to_query {
            match self.fetch_collateral_balance(&asset).await {
                Ok(balance) => {
                    if let Some(entry) = self.collateral_inventory.get_mut(&asset) {
                        entry.update_balance(CollateralAssetAmount::from(balance.0));
                        refreshed += 1;

                        if balance.0 > 0 {
                            non_zero_balances.insert(asset.to_string(), balance);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        collateral_asset = %asset,
                        error = %e,
                        "Failed to fetch collateral balance"
                    );
                    errors += 1;
                }
            }
        }

        if non_zero_balances.is_empty() {
            tracing::info!(
                refreshed = refreshed,
                errors = errors,
                "Collateral asset inventory refresh complete - no holdings"
            );
        } else {
            let assets_str = non_zero_balances
                .iter()
                .map(|(asset, balance)| format!("{}: {}", asset, balance.0))
                .collect::<Vec<_>>()
                .join(", ");

            tracing::info!(
                refreshed = refreshed,
                errors = errors,
                collateral_holdings = assets_str,
                "Collateral asset inventory refresh complete"
            );
        }

        Ok(non_zero_balances)
    }

    /// Fetches current balance for a collateral asset from blockchain
    async fn fetch_collateral_balance(
        &self,
        asset: &FungibleAsset<CollateralAsset>,
    ) -> InventoryResult<U128> {
        let result = self
            .client
            .read(token::GetBalanceOf {
                token: TokenReference::from(asset),
                account_id: self.account_id.clone(),
            })
            .await
            .map_err(|e| InventoryError::FetchBalanceError(e.into()))?;

        Ok(U128(result.balance.0))
    }

    /// Gets collateral inventory for iteration
    pub fn collateral_holdings(&self) -> Vec<(FungibleAsset<CollateralAsset>, U128)> {
        self.collateral_inventory
            .iter()
            .filter_map(|(asset, entry)| {
                let balance_u128 = u128::from(entry.balance);
                if balance_u128 > 0 {
                    Some((asset.clone(), U128(balance_u128)))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Gets current collateral balances without refreshing from RPC
    ///
    /// Returns a `HashMap` of asset string -> balance for assets with non-zero balance.
    /// This is useful when you just want to check what's in memory without making RPC calls.
    pub fn get_collateral_balances(&self) -> HashMap<String, U128> {
        self.collateral_inventory
            .iter()
            .filter_map(|(asset, entry)| {
                let balance_u128 = u128::from(entry.balance);
                if balance_u128 > 0 {
                    Some((asset.to_string(), U128(balance_u128)))
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Snapshot of inventory state for logging/metrics
#[derive(Debug, Clone, Serialize)]
pub struct InventorySnapshot {
    pub entries: Vec<InventorySnapshotEntry>,
}

#[derive(Debug, Clone, Serialize)]
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

    /// Builds a [`SigningClient`] for tests. No network I/O happens at
    /// construction; the inventory unit tests only exercise in-memory
    /// reserve/release accounting and never issue reads.
    fn create_test_client() -> SigningClient {
        let network = near_api::NetworkConfig::from_rpc_url(
            "testnet",
            "https://rpc.testnet.near.org".parse().unwrap(),
        );
        // Derive a self-consistent ed25519 keypair so `SigningClient::connect`
        // (which validates the secret against its embedded public key) accepts it.
        let secret_key: near_api::SecretKey =
            near_crypto::SecretKey::from_seed(near_crypto::KeyType::ED25519, "liquidator-test")
                .to_string()
                .parse()
                .unwrap();
        SigningClient::connect(
            network,
            AccountId::from_str("test.near").unwrap(),
            secret_key,
        )
        .unwrap()
    }

    #[test]
    fn test_inventory_entry_reserve_release() {
        let mut entry: InventoryEntry<BorrowAsset> = InventoryEntry {
            balance: BorrowAssetAmount::from(1000),
            reserved: BorrowAssetAmount::from(0),
            last_updated: Instant::now(),
        };

        // Initial state
        assert_eq!(u128::from(entry.available()), 1000);

        // Reserve 300
        entry.reserve(BorrowAssetAmount::from(300)).unwrap();
        assert_eq!(u128::from(entry.available()), 700);
        assert_eq!(u128::from(entry.reserved), 300);

        // Reserve another 200
        entry.reserve(BorrowAssetAmount::from(200)).unwrap();
        assert_eq!(u128::from(entry.available()), 500);
        assert_eq!(u128::from(entry.reserved), 500);

        // Try to reserve more than available
        let result = entry.reserve(BorrowAssetAmount::from(600));
        assert!(result.is_err());

        // Release 300
        entry.release(BorrowAssetAmount::from(300));
        assert_eq!(u128::from(entry.available()), 800);
        assert_eq!(u128::from(entry.reserved), 200);

        // Release remaining
        entry.release(BorrowAssetAmount::from(200));
        assert_eq!(u128::from(entry.available()), 1000);
        assert_eq!(u128::from(entry.reserved), 0);
    }

    #[test]
    fn test_inventory_manager_reserve_release() {
        let client = create_test_client();
        let account_id = AccountId::from_str("test.near").unwrap();
        let mut inventory = InventoryManager::new(client, account_id);

        let asset = create_test_asset();

        // Add asset manually
        inventory.inventory.insert(
            asset.clone(),
            InventoryEntry {
                balance: BorrowAssetAmount::from(1000),
                reserved: BorrowAssetAmount::from(0),
                last_updated: Instant::now(),
            },
        );

        // Check available balance
        assert_eq!(inventory.get_available_balance(&asset).0, 1000);

        // Reserve 300
        inventory
            .reserve(&asset, BorrowAssetAmount::from(300))
            .unwrap();
        assert_eq!(inventory.get_available_balance(&asset).0, 700);
        assert_eq!(inventory.get_reserved_balance(&asset).0, 300);

        // Release 100
        inventory.release(&asset, BorrowAssetAmount::from(100));
        assert_eq!(inventory.get_available_balance(&asset).0, 800);
        assert_eq!(inventory.get_reserved_balance(&asset).0, 200);
    }

    #[test]
    fn test_inventory_reserve_insufficient_balance() {
        let client = create_test_client();
        let account_id = AccountId::from_str("test.near").unwrap();
        let mut inventory = InventoryManager::new(client, account_id);

        let asset = create_test_asset();

        inventory.inventory.insert(
            asset.clone(),
            InventoryEntry {
                balance: BorrowAssetAmount::from(100),
                reserved: BorrowAssetAmount::from(0),
                last_updated: Instant::now(),
            },
        );

        // Try to reserve more than available
        let result = inventory.reserve(&asset, BorrowAssetAmount::from(200));
        assert!(result.is_err());
    }

    #[test]
    fn test_inventory_get_total_balance() {
        let client = create_test_client();
        let account_id = AccountId::from_str("test.near").unwrap();
        let mut inventory = InventoryManager::new(client, account_id);

        let asset = create_test_asset();

        inventory.inventory.insert(
            asset.clone(),
            InventoryEntry {
                balance: BorrowAssetAmount::from(1000),
                reserved: BorrowAssetAmount::from(300),
                last_updated: Instant::now(),
            },
        );

        assert_eq!(inventory.get_total_balance(&asset).0, 1000);
        assert_eq!(inventory.get_available_balance(&asset).0, 700);
        assert_eq!(inventory.get_reserved_balance(&asset).0, 300);
    }

    #[test]
    fn test_collateral_balances_empty() {
        let client = create_test_client();
        let account_id = AccountId::from_str("test.near").unwrap();
        let inventory = InventoryManager::new(client, account_id);

        let balances = inventory.get_collateral_balances();
        assert!(balances.is_empty());
    }
}
