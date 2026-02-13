//! Manual reconciliation helpers for external assets.
//!
//! `resync_external_assets` runs a privileged refresh flow and emits
//! `ReconciliationEvent` entries for audit. Requires `ActionKind::ManualReconcile`.

use alloc::string::String;
use alloc::vec::Vec;

use soroban_sdk::{Address as SdkAddress, Env};
use templar_vault_kernel::{Address as KernelAddress, AssetId, TargetId, TimestampNs};

use crate::auth::{ActionKind, AuthAdapter, AuthError};
use crate::error::RuntimeError;
use crate::market::{MarketAdapter, MarketRef, SorobanMarketAdapter};

// ---------------------------------------------------------------------------
// Audit Events
// ---------------------------------------------------------------------------

/// Audit event types for reconciliation operations.
///
/// These events are emitted during reconciliation to provide an audit trail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconciliationEvent {
    /// Reconciliation started.
    Started {
        op_id: u64,
        caller: KernelAddress,
        timestamp_ns: TimestampNs,
        market_count: u32,
    },
    /// Market principal read.
    MarketRead {
        op_id: u64,
        market_id: TargetId,
        principal: i128,
    },
    /// External assets synced to kernel.
    AssetsSync {
        op_id: u64,
        old_external_assets: u128,
        new_external_assets: u128,
    },
    /// Reconciliation completed successfully.
    Completed {
        op_id: u64,
        timestamp_ns: TimestampNs,
        markets_refreshed: u32,
        final_external_assets: u128,
    },
    /// Reconciliation failed.
    Failed {
        op_id: u64,
        timestamp_ns: TimestampNs,
        error: String,
    },
}

impl ReconciliationEvent {
    /// Create a Started event.
    #[inline]
    #[must_use]
    pub const fn started(
        op_id: u64,
        caller: KernelAddress,
        timestamp_ns: TimestampNs,
        market_count: u32,
    ) -> Self {
        Self::Started {
            op_id,
            caller,
            timestamp_ns,
            market_count,
        }
    }

    /// Create a MarketRead event.
    #[inline]
    #[must_use]
    pub const fn market_read(op_id: u64, market_id: TargetId, principal: i128) -> Self {
        Self::MarketRead {
            op_id,
            market_id,
            principal,
        }
    }

    /// Create an AssetsSync event.
    #[inline]
    #[must_use]
    pub const fn assets_sync(
        op_id: u64,
        old_external_assets: u128,
        new_external_assets: u128,
    ) -> Self {
        Self::AssetsSync {
            op_id,
            old_external_assets,
            new_external_assets,
        }
    }

    /// Create a Completed event.
    #[inline]
    #[must_use]
    pub const fn completed(
        op_id: u64,
        timestamp_ns: TimestampNs,
        markets_refreshed: u32,
        final_external_assets: u128,
    ) -> Self {
        Self::Completed {
            op_id,
            timestamp_ns,
            markets_refreshed,
            final_external_assets,
        }
    }

    /// Create a Failed event.
    #[inline]
    #[must_use]
    pub fn failed(op_id: u64, timestamp_ns: TimestampNs, error: impl Into<String>) -> Self {
        Self::Failed {
            op_id,
            timestamp_ns,
            error: error.into(),
        }
    }

    /// Get the operation ID for this event.
    #[inline]
    #[must_use]
    pub const fn op_id(&self) -> u64 {
        match self {
            Self::Started { op_id, .. }
            | Self::MarketRead { op_id, .. }
            | Self::AssetsSync { op_id, .. }
            | Self::Completed { op_id, .. }
            | Self::Failed { op_id, .. } => *op_id,
        }
    }
}

// ---------------------------------------------------------------------------
// Reconciliation Record
// ---------------------------------------------------------------------------

/// Summary record for a manual reconciliation run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationRecord {
    pub op_id: u64,
    pub markets_refreshed: u32,
    pub new_external_assets: u128,
}

impl ReconciliationRecord {
    /// Create a new reconciliation record.
    #[inline]
    #[must_use]
    pub const fn new(op_id: u64, markets_refreshed: u32, new_external_assets: u128) -> Self {
        Self {
            op_id,
            markets_refreshed,
            new_external_assets,
        }
    }
}

// ---------------------------------------------------------------------------
// Resync Request and Result
// ---------------------------------------------------------------------------

/// Request parameters for the `resync_external_assets` entrypoint.
#[derive(Clone, Debug)]
pub struct ResyncRequest {
    /// Caller address (kernel format for auth).
    pub caller: KernelAddress,
    /// Operation ID for the refresh flow.
    pub op_id: u64,
    /// List of market target IDs to refresh.
    pub market_ids: Vec<TargetId>,
    /// Asset contract address (SDK format for market adapter calls).
    pub asset: SdkAddress,
    /// Current external assets value (for delta calculation).
    pub current_external_assets: u128,
}

impl ResyncRequest {
    /// Create a new resync request.
    #[inline]
    #[must_use]
    pub fn new(
        caller: KernelAddress,
        op_id: u64,
        market_ids: Vec<TargetId>,
        asset: SdkAddress,
        current_external_assets: u128,
    ) -> Self {
        Self {
            caller,
            op_id,
            market_ids,
            asset,
            current_external_assets,
        }
    }
}

/// Result of the `resync_external_assets` entrypoint.
#[derive(Clone, Debug)]
pub struct ResyncResult {
    /// Reconciliation record with summary.
    pub record: ReconciliationRecord,
    /// Audit events emitted during reconciliation.
    pub events: Vec<ReconciliationEvent>,
    /// Delta between old and new external assets (signed).
    pub delta: i128,
}

impl ResyncResult {
    /// Create a new resync result.
    #[inline]
    #[must_use]
    pub fn new(
        record: ReconciliationRecord,
        events: Vec<ReconciliationEvent>,
        delta: i128,
    ) -> Self {
        Self {
            record,
            events,
            delta,
        }
    }
}

// ---------------------------------------------------------------------------
// Privileged Resync Entrypoint
// ---------------------------------------------------------------------------

/// Privileged manual reconciliation entrypoint.
///
/// Runs the full refresh flow:
/// 1. Authorize caller for `ManualReconcile` action.
/// 2. Emit `ReconciliationEvent::Started`.
/// 3. Read principals from each market in the plan.
/// 4. Aggregate principals to compute new external assets.
/// 5. Emit `ReconciliationEvent::AssetsSync` with old/new values.
/// 6. Emit `ReconciliationEvent::Completed`.
///
/// On failure, emits `ReconciliationEvent::Failed` and returns error.
///
/// # Security
///
/// This function requires the caller to be authorized for `ActionKind::ManualReconcile`,
/// which should be restricted to owner/guardian roles.
///
/// # Arguments
///
/// * `env` - The Soroban environment.
/// * `auth` - The auth adapter for authorization checks.
/// * `market_adapter` - The market adapter for reading principals.
/// * `request` - The resync request parameters.
///
/// # Returns
///
/// `Ok(ResyncResult)` with the reconciliation record and emitted events,
/// or `Err(RuntimeError)` if authorization or market reads fail.
///
/// # Example
///
/// ```ignore
/// let request = ResyncRequest::new(
///     caller_address,
///     op_id,
///     vec![market_1, market_2],
///     asset_address,
///     current_external_assets,
/// );
///
/// let result = resync_external_assets(&env, &auth, &adapter, request)?;
/// // Apply new_external_assets to kernel state
/// ```
pub fn resync_external_assets<A: AuthAdapter, M: SorobanMarketAdapter>(
    env: &Env,
    auth: &A,
    market_adapter: &M,
    request: ResyncRequest,
) -> Result<ResyncResult, RuntimeError> {
    let timestamp_ns = env.ledger().timestamp() * 1_000_000_000;
    let market_count = request.market_ids.len() as u32;
    let mut events = Vec::new();

    // 1. Authorize caller
    auth.authorize(ActionKind::ManualReconcile, request.caller, None)
        .map_err(|e| match e {
            AuthError::NotAuthorized { .. } => {
                RuntimeError::unauthorized("caller not authorized for ManualReconcile")
            }
            AuthError::InvalidProof => RuntimeError::unauthorized("invalid proof"),
            AuthError::MissingRole(role) => {
                RuntimeError::unauthorized(alloc::format!("missing role: {}", role))
            }
            AuthError::VaultPaused => RuntimeError::invalid_state("vault is paused"),
        })?;

    // 2. Emit Started event
    events.push(ReconciliationEvent::started(
        request.op_id,
        request.caller,
        timestamp_ns,
        market_count,
    ));

    // 3. Read principals from each market
    let mut total_principals: i128 = 0;
    for &market_id in &request.market_ids {
        let principal = market_adapter
            .total_assets(env, &request.asset)
            .map_err(|e| {
                events.push(ReconciliationEvent::failed(
                    request.op_id,
                    timestamp_ns,
                    alloc::format!("failed to read market {}: {:?}", market_id, e),
                ));
                e
            })?;

        events.push(ReconciliationEvent::market_read(
            request.op_id,
            market_id,
            principal,
        ));

        total_principals = total_principals
            .checked_add(principal)
            .ok_or(RuntimeError::invalid_state("total principals overflow"))?;
    }

    // 4. Convert to u128 (principals should be non-negative after aggregation)
    let new_external_assets = if total_principals >= 0 {
        total_principals as u128
    } else {
        // Log negative total as anomaly (could indicate accounting error)
        events.push(ReconciliationEvent::failed(
            request.op_id,
            timestamp_ns,
            alloc::format!("negative total principals: {}", total_principals),
        ));
        return Err(RuntimeError::invalid_state("negative total principals"));
    };

    // 5. Emit AssetsSync event
    events.push(ReconciliationEvent::assets_sync(
        request.op_id,
        request.current_external_assets,
        new_external_assets,
    ));

    // 6. Calculate delta (safe: both values are u128 that passed non-negative check,
    //    and the difference of two non-negative values fits in i128)
    let new_i128 = i128::try_from(new_external_assets)
        .map_err(|_| RuntimeError::invalid_state("new_external_assets exceeds i128"))?;
    let old_i128 = i128::try_from(request.current_external_assets)
        .map_err(|_| RuntimeError::invalid_state("current_external_assets exceeds i128"))?;
    let delta = new_i128
        .checked_sub(old_i128)
        .ok_or(RuntimeError::invalid_state("external assets delta overflow"))?;

    // 7. Emit Completed event
    events.push(ReconciliationEvent::completed(
        request.op_id,
        timestamp_ns,
        market_count,
        new_external_assets,
    ));

    // 8. Return result
    let record = ReconciliationRecord::new(request.op_id, market_count, new_external_assets);
    Ok(ResyncResult::new(record, events, delta))
}

// ---------------------------------------------------------------------------
// Lower-level helpers (chain-agnostic)
// ---------------------------------------------------------------------------

/// Aggregate external assets by querying each market in the refresh plan.
///
/// The caller is responsible for authorization and for passing the resulting
/// `new_external_assets` into kernel `SyncExternalAssets` before `FinishRefreshing`.
///
/// # Arguments
///
/// * `adapter` - The market adapter for reading principals.
/// * `op_id` - The operation ID.
/// * `plan` - The list of market references to query.
///
/// # Returns
///
/// A `ReconciliationRecord` with the aggregated external assets.
pub fn reconcile_external_assets<A: MarketAdapter>(
    adapter: &A,
    op_id: u64,
    plan: &[MarketRef],
) -> Result<ReconciliationRecord, RuntimeError> {
    let mut total = 0u128;
    for market in plan {
        let assets = adapter.total_assets(market.clone())?;
        total = total
            .checked_add(assets)
            .ok_or(RuntimeError::invalid_state("external assets total overflow"))?;
    }

    Ok(ReconciliationRecord {
        op_id,
        markets_refreshed: plan.len() as u32,
        new_external_assets: total,
    })
}

/// Helper to build a refresh plan from raw market ids + a shared asset id.
///
/// # Arguments
///
/// * `asset_id` - The asset identifier.
/// * `markets` - The list of market target IDs.
///
/// # Returns
///
/// A vector of `MarketRef` entries for the refresh plan.
pub fn build_refresh_plan(asset_id: AssetId, markets: &[TargetId]) -> Vec<MarketRef> {
    markets
        .iter()
        .map(|market_id| (*market_id, asset_id.clone()).into())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
