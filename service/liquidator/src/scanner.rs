//! Market position scanner module.
//!
//! Handles scanning markets for borrow positions and checking liquidation status.

use near_jsonrpc_client::JsonRpcClient;
use near_sdk::{serde_json::json, AccountId};
use std::collections::HashMap;
use templar_common::{
    borrow::{BorrowPosition, BorrowStatus},
    oracle::pyth::OracleResponse,
};
use tracing::{debug, info};

use crate::{
    rpc::{view, RpcError},
    LiquidatorError, LiquidatorResult,
};

/// Type alias for borrow positions map
pub type BorrowPositions = HashMap<AccountId, BorrowPosition>;

/// Market position scanner.
///
/// Responsible for:
/// - Fetching all borrow positions from a market
/// - Checking liquidation status of positions
/// - Pagination handling for large markets
/// - Market version compatibility checking (NEP-330)
pub struct MarketScanner {
    client: JsonRpcClient,
    market: AccountId,
}

impl MarketScanner {
    /// Minimum supported contract version (semver).
    /// Markets with version < 1.0.0 will be skipped.
    pub const MIN_SUPPORTED_VERSION: (u32, u32, u32) = (1, 0, 0);

    /// Minimum version that supports partial liquidation (semver).
    /// Markets with version < 1.1.0 only support full liquidation.
    pub const MIN_PARTIAL_LIQUIDATION_VERSION: (u32, u32, u32) = (1, 1, 0);
}

impl MarketScanner {
    /// Creates a new market scanner.
    pub fn new(client: JsonRpcClient, market: AccountId) -> Self {
        Self { client, market }
    }

    /// Fetches borrow status for an account.
    #[tracing::instrument(skip(self, oracle_response), level = "debug")]
    pub async fn get_borrow_status(
        &self,
        account_id: &AccountId,
        oracle_response: &OracleResponse,
    ) -> Result<Option<BorrowStatus>, RpcError> {
        view(
            &self.client,
            self.market.clone(),
            "get_borrow_status",
            &json!({
                "account_id": account_id,
                "oracle_response": oracle_response,
            }),
        )
        .await
    }

    /// Fetches all borrow positions from the market with pagination.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn get_all_borrows(&self) -> LiquidatorResult<BorrowPositions> {
        let mut all_positions: BorrowPositions = HashMap::new();
        let page_size = 500;
        let mut current_offset = 0;

        loop {
            let page: BorrowPositions = view(
                &self.client,
                self.market.clone(),
                "list_borrow_positions",
                json!({
                    "offset": current_offset,
                    "count": page_size,
                }),
            )
            .await
            .map_err(LiquidatorError::ListBorrowPositionsError)?;

            let fetched = page.len();
            if fetched == 0 {
                break;
            }

            debug!(
                market = %self.market,
                offset = current_offset,
                fetched = fetched,
                "Fetched borrow positions page"
            );

            all_positions.extend(page);
            current_offset += fetched;

            if fetched < page_size {
                break;
            }
        }

        info!(
            market = %self.market,
            total_positions = all_positions.len(),
            "Fetched all borrow positions"
        );

        Ok(all_positions)
    }

    /// Checks if a position is liquidatable.
    ///
    /// Returns `Some(reason)` if the position is liquidatable with the liquidation reason,
    /// or `None` if the position is not liquidatable.
    ///
    /// # Errors
    ///
    /// Returns an error if the borrow status cannot be fetched
    pub async fn is_liquidatable(
        &self,
        account_id: &AccountId,
        oracle_response: &OracleResponse,
    ) -> LiquidatorResult<Option<String>> {
        let status = self
            .get_borrow_status(account_id, oracle_response)
            .await
            .map_err(LiquidatorError::FetchBorrowStatus)?;

        match status {
            Some(BorrowStatus::Liquidation(reason)) => Ok(Some(format!("{reason:?}"))),
            Some(_) | None => Ok(None),
        }
    }

    /// Checks market compatibility and feature support in a single call.
    ///
    /// This method fetches the version once and checks:
    /// 1. Basic compatibility (version >= 1.0.0)
    /// 2. Partial liquidation support (version >= 1.1.0) if required by strategy
    ///
    /// # Arguments
    ///
    /// * `requires_partial_liquidation` - Whether the strategy requires partial liquidation support
    ///
    /// # Returns
    ///
    /// `Ok(())` if the market is compatible with the strategy requirements.
    ///
    /// # Errors
    ///
    /// Returns an error if the market version is not supported or doesn't support
    /// required features.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn check_market_compatibility(
        &self,
        requires_partial_liquidation: bool,
    ) -> LiquidatorResult<()> {
        use crate::rpc::get_contract_version;

        let Some(version_string) = get_contract_version(&self.client, &self.market).await else {
            // No NEP-330 metadata - can't verify version
            if requires_partial_liquidation {
                info!(
                    market = %self.market,
                    "Contract missing NEP-330 metadata, cannot verify partial liquidation support"
                );
                return Err(LiquidatorError::StrategyError(
                    "Contract missing version metadata, cannot verify partial liquidation support"
                        .to_string(),
                ));
            }
            debug!(
                market = %self.market,
                "Contract missing NEP-330 metadata, assuming basic compatibility"
            );
            return Ok(());
        };

        // Parse semver (e.g., "1.2.3" or "0.1.0")
        let parts: Vec<&str> = version_string.split('.').collect();
        let (major, minor, patch) = if let [maj, min, pat] = parts.as_slice() {
            let major = maj.parse::<u32>().unwrap_or(0);
            let minor = min.parse::<u32>().unwrap_or(0);
            let patch = pat.parse::<u32>().unwrap_or(0);
            (major, minor, patch)
        } else {
            info!(
                market = %self.market,
                version = %version_string,
                "Invalid semver format, skipping market"
            );
            return Err(LiquidatorError::StrategyError(format!(
                "Invalid version format: {version_string}"
            )));
        };

        // Check basic compatibility
        let is_compatible = (major, minor, patch) >= Self::MIN_SUPPORTED_VERSION;
        if !is_compatible {
            let (min_major, min_minor, min_patch) = Self::MIN_SUPPORTED_VERSION;
            info!(
                market = %self.market,
                version = %version_string,
                min_version = %format!("{min_major}.{min_minor}.{min_patch}"),
                "Skipping market - unsupported contract version"
            );
            return Err(LiquidatorError::StrategyError(format!(
                "Market version {version_string} < {min_major}.{min_minor}.{min_patch}"
            )));
        }

        // Check partial liquidation support if required
        if requires_partial_liquidation {
            let supports_partial = (major, minor, patch) >= Self::MIN_PARTIAL_LIQUIDATION_VERSION;
            if !supports_partial {
                let (min_major, min_minor, min_patch) = Self::MIN_PARTIAL_LIQUIDATION_VERSION;
                info!(
                    market = %self.market,
                    version = %version_string,
                    min_version = %format!("{min_major}.{min_minor}.{min_patch}"),
                    "Skipping market - does not support partial liquidation"
                );
                return Err(LiquidatorError::StrategyError(format!(
                    "Market version {version_string} does not support partial liquidation (requires {min_major}.{min_minor}.{min_patch}+)"
                )));
            }
        }

        info!(
            market = %self.market,
            version = %version_string,
            "Market is compatible"
        );
        Ok(())
    }

    /// Tests if the market is compatible by verifying its version via NEP-330.
    ///
    /// # Errors
    ///
    /// Returns an error if the market version is not supported.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn test_market_compatibility(&self) -> LiquidatorResult<()> {
        self.check_market_compatibility(false).await
    }

    /// Checks if the market supports partial liquidation based on its version.
    ///
    /// Markets with version >= 1.1.0 support partial liquidation.
    /// Older markets only support full liquidation of all liquidatable collateral.
    ///
    /// # Returns
    ///
    /// `true` if the market supports partial liquidation, `false` otherwise.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn supports_partial_liquidation(&self) -> bool {
        self.check_market_compatibility(true).await.is_ok()
    }

    /// Gets the market version via NEP-330 contract metadata.
    ///
    /// Fetches the contract version and parses it as a semver tuple.
    /// Used to enable version-specific liquidation logic (v1.0 vs v1.1+).
    ///
    /// # Returns
    ///
    /// `Some((major, minor, patch))` if version metadata is available and parseable,
    /// `None` if the contract doesn't support NEP-330 or version format is invalid.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let version = scanner.get_market_version().await;
    /// match version {
    ///     Some((1, 0, 0)) => println!("v1.0 market"),
    ///     Some((1, 1, _)) => println!("v1.1+ market"),
    ///     None => println!("Unknown version"),
    /// }
    /// ```
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn get_market_version(&self) -> Option<(u32, u32, u32)> {
        use crate::rpc::get_contract_version;

        let version_string = get_contract_version(&self.client, &self.market).await?;

        // Parse semver (e.g., "1.2.3" or "0.1.0")
        let parts: Vec<&str> = version_string.split('.').collect();
        if let [maj, min, pat] = parts.as_slice() {
            let major = maj.parse::<u32>().ok()?;
            let minor = min.parse::<u32>().ok()?;
            let patch = pat.parse::<u32>().ok()?;
            Some((major, minor, patch))
        } else {
            None
        }
    }
}
