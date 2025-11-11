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

    /// Tests if the market is compatible by verifying its version via NEP-330.
    ///
    /// # Errors
    ///
    /// Returns an error if the market version is not supported, including the
    /// actual version and minimum required version in the error message.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn test_market_compatibility(&self) -> LiquidatorResult<()> {
        use crate::rpc::get_contract_version;

        let Some(version_string) = get_contract_version(&self.client, &self.market).await else {
            info!(
                market = %self.market,
                "Contract does not implement NEP-330 (contract_source_metadata), assuming compatible"
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
                "Invalid semver format, assuming compatible"
            );
            return Ok(());
        };

        let is_compatible = (major, minor, patch) >= Self::MIN_SUPPORTED_VERSION;

        if is_compatible {
            info!(
                market = %self.market,
                version = %version_string,
                "Market is compatible and supported"
            );
            Ok(())
        } else {
            let (min_major, min_minor, min_patch) = Self::MIN_SUPPORTED_VERSION;
            let error_msg = format!(
                "Market version {version_string} is not supported (minimum required: {min_major}.{min_minor}.{min_patch})"
            );
            info!(
                market = %self.market,
                version = %version_string,
                min_version = %format!("{min_major}.{min_minor}.{min_patch}"),
                "Skipping market - unsupported contract version"
            );
            Err(LiquidatorError::StrategyError(error_msg))
        }
    }
}
