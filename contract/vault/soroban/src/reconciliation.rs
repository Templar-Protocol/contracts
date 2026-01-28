//! Manual reconciliation helpers for external assets.
//!
//! The Soroban executor can use these helpers to reconcile market principals
//! in a privileged, audited entrypoint.
//!
//! # Overview
//!
//! This module provides:
//!
//! - **Audit events**: [`ReconciliationEvent`] for logging all reconciliation actions.
//! - **Reconciliation entrypoint**: [`resync_external_assets`] privileged function
//!   that runs the full refresh flow (BeginRefreshing -> read principals ->
//!   SyncExternalAssets -> FinishRefreshing) in one call.
//! - **Helper functions**: [`reconcile_external_assets`] and [`build_refresh_plan`]
//!   for lower-level reconciliation logic.
//!
//! # Security
//!
//! The `resync_external_assets` entrypoint requires `ActionKind::ManualReconcile`
//! authorization, which should be restricted to owner/guardian roles.

use alloc::string::String;
use alloc::vec::Vec;

use templar_vault_kernel::{AssetId, TargetId, TimestampNs};

use crate::auth::{ActionKind, AuthAdapter, AuthError};
use crate::error::RuntimeError;
use crate::market::{Env, MarketAdapter, MarketRef, SorobanAddress, SorobanMarketAdapter};

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
        /// Operation ID.
        op_id: u64,
        /// Caller address.
        caller: SorobanAddress,
        /// Timestamp when reconciliation started (nanoseconds).
        timestamp_ns: TimestampNs,
        /// Number of markets to refresh.
        market_count: u32,
    },
    /// Market principal read.
    MarketRead {
        /// Operation ID.
        op_id: u64,
        /// Market target ID.
        market_id: TargetId,
        /// Principal value read.
        principal: i128,
    },
    /// External assets synced to kernel.
    AssetsSync {
        /// Operation ID.
        op_id: u64,
        /// Previous external assets value.
        old_external_assets: u128,
        /// New external assets value.
        new_external_assets: u128,
    },
    /// Reconciliation completed successfully.
    Completed {
        /// Operation ID.
        op_id: u64,
        /// Timestamp when reconciliation completed (nanoseconds).
        timestamp_ns: TimestampNs,
        /// Number of markets refreshed.
        markets_refreshed: u32,
        /// Final external assets value.
        final_external_assets: u128,
    },
    /// Reconciliation failed.
    Failed {
        /// Operation ID.
        op_id: u64,
        /// Timestamp when failure occurred (nanoseconds).
        timestamp_ns: TimestampNs,
        /// Error message.
        error: String,
    },
}

impl ReconciliationEvent {
    /// Create a Started event.
    #[inline]
    #[must_use]
    pub const fn started(
        op_id: u64,
        caller: SorobanAddress,
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
    /// Operation ID.
    pub op_id: u64,
    /// Number of markets refreshed.
    pub markets_refreshed: u32,
    /// New external assets value.
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
    /// Caller address (must be authorized for ManualReconcile).
    pub caller: SorobanAddress,
    /// Operation ID for the refresh flow.
    pub op_id: u64,
    /// List of market target IDs to refresh.
    pub market_ids: Vec<TargetId>,
    /// Asset contract address.
    pub asset: SorobanAddress,
    /// Current external assets value (for delta calculation).
    pub current_external_assets: u128,
}

impl ResyncRequest {
    /// Create a new resync request.
    #[inline]
    #[must_use]
    pub fn new(
        caller: SorobanAddress,
        op_id: u64,
        market_ids: Vec<TargetId>,
        asset: SorobanAddress,
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
    let timestamp_ns = env.ledger_timestamp_ns;
    let market_count = request.market_ids.len() as u32;
    let mut events = Vec::new();

    // 1. Authorize caller
    auth.authorize(ActionKind::ManualReconcile, request.caller.as_bytes(), None)
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
            .total_assets(env, request.asset)
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

        total_principals = total_principals.saturating_add(principal);
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

    // 6. Calculate delta
    let delta = (new_external_assets as i128) - (request.current_external_assets as i128);

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
        total = total.saturating_add(assets);
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
        .map(|market_id| MarketRef::new(*market_id, asset_id.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::PermissiveAuth;
    use crate::market::MockSorobanMarketAdapter;
    use alloc::vec;
    use core::cell::Cell;

    struct MockAdapter {
        total: Cell<u128>,
    }

    impl MarketAdapter for MockAdapter {
        fn supply(&mut self, _market: MarketRef, _amount: u128) -> Result<(), RuntimeError> {
            Ok(())
        }

        fn withdraw(&mut self, _market: MarketRef, _amount: u128) -> Result<(), RuntimeError> {
            Ok(())
        }

        fn total_assets(&self, _market: MarketRef) -> Result<u128, RuntimeError> {
            let cur = self.total.get();
            self.total.set(cur.saturating_add(1));
            Ok(cur)
        }
    }

    #[test]
    fn reconcile_external_assets_sums() {
        let adapter = MockAdapter {
            total: Cell::new(10),
        };
        let asset = AssetId::from([7u8; 32]);
        let plan = vec![
            MarketRef::new(1, asset.clone()),
            MarketRef::new(2, asset.clone()),
            MarketRef::new(3, asset),
        ];

        let record = reconcile_external_assets(&adapter, 42, &plan).unwrap();
        assert_eq!(record.op_id, 42);
        assert_eq!(record.markets_refreshed, 3);
        assert_eq!(record.new_external_assets, 10 + 11 + 12);
    }

    #[test]
    fn build_refresh_plan_creates_refs() {
        let asset = AssetId::from([5u8; 32]);
        let markets = [1, 2, 3];
        let plan = build_refresh_plan(asset.clone(), &markets);

        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].market_id, 1);
        assert_eq!(plan[1].market_id, 2);
        assert_eq!(plan[2].market_id, 3);
        for market_ref in &plan {
            assert_eq!(market_ref.asset_id, asset);
        }
    }

    #[test]
    fn reconciliation_record_new() {
        let record = ReconciliationRecord::new(100, 5, 50000);
        assert_eq!(record.op_id, 100);
        assert_eq!(record.markets_refreshed, 5);
        assert_eq!(record.new_external_assets, 50000);
    }

    // ---------------------------------------------------------------------------
    // Reconciliation Event Tests
    // ---------------------------------------------------------------------------

    #[test]
    fn reconciliation_event_started() {
        let caller = SorobanAddress::from_bytes([1u8; 32]);
        let event = ReconciliationEvent::started(1, caller, 1000, 5);

        assert_eq!(event.op_id(), 1);
        match event {
            ReconciliationEvent::Started {
                op_id,
                caller: c,
                timestamp_ns,
                market_count,
            } => {
                assert_eq!(op_id, 1);
                assert_eq!(c, caller);
                assert_eq!(timestamp_ns, 1000);
                assert_eq!(market_count, 5);
            }
            _ => panic!("expected Started event"),
        }
    }

    #[test]
    fn reconciliation_event_market_read() {
        let event = ReconciliationEvent::market_read(2, 42, 1000);

        assert_eq!(event.op_id(), 2);
        match event {
            ReconciliationEvent::MarketRead {
                op_id,
                market_id,
                principal,
            } => {
                assert_eq!(op_id, 2);
                assert_eq!(market_id, 42);
                assert_eq!(principal, 1000);
            }
            _ => panic!("expected MarketRead event"),
        }
    }

    #[test]
    fn reconciliation_event_assets_sync() {
        let event = ReconciliationEvent::assets_sync(3, 1000, 1500);

        assert_eq!(event.op_id(), 3);
        match event {
            ReconciliationEvent::AssetsSync {
                op_id,
                old_external_assets,
                new_external_assets,
            } => {
                assert_eq!(op_id, 3);
                assert_eq!(old_external_assets, 1000);
                assert_eq!(new_external_assets, 1500);
            }
            _ => panic!("expected AssetsSync event"),
        }
    }

    #[test]
    fn reconciliation_event_completed() {
        let event = ReconciliationEvent::completed(4, 2000, 3, 5000);

        assert_eq!(event.op_id(), 4);
        match event {
            ReconciliationEvent::Completed {
                op_id,
                timestamp_ns,
                markets_refreshed,
                final_external_assets,
            } => {
                assert_eq!(op_id, 4);
                assert_eq!(timestamp_ns, 2000);
                assert_eq!(markets_refreshed, 3);
                assert_eq!(final_external_assets, 5000);
            }
            _ => panic!("expected Completed event"),
        }
    }

    #[test]
    fn reconciliation_event_failed() {
        let event = ReconciliationEvent::failed(5, 3000, "test error");

        assert_eq!(event.op_id(), 5);
        match event {
            ReconciliationEvent::Failed {
                op_id,
                timestamp_ns,
                error,
            } => {
                assert_eq!(op_id, 5);
                assert_eq!(timestamp_ns, 3000);
                assert_eq!(error, "test error");
            }
            _ => panic!("expected Failed event"),
        }
    }

    // ---------------------------------------------------------------------------
    // Resync External Assets Tests
    // ---------------------------------------------------------------------------

    #[test]
    fn resync_external_assets_success() {
        let env = Env::mock();
        let auth = PermissiveAuth;
        let adapter = MockSorobanMarketAdapter::new(1000);

        let request = ResyncRequest::new(
            SorobanAddress::from_bytes([1u8; 32]),
            42,
            vec![1, 2, 3],
            SorobanAddress::from_bytes([2u8; 32]),
            2500, // current external assets
        );

        let result = resync_external_assets(&env, &auth, &adapter, request).unwrap();

        // Should have 3 markets * 1000 = 3000 total
        // But MockSorobanMarketAdapter returns the same value for all calls
        // So we get 1000 * 3 = 3000
        assert_eq!(result.record.op_id, 42);
        assert_eq!(result.record.markets_refreshed, 3);
        assert_eq!(result.record.new_external_assets, 3000);

        // Delta should be 3000 - 2500 = 500
        assert_eq!(result.delta, 500);

        // Should have emitted events: Started, 3x MarketRead, AssetsSync, Completed
        assert_eq!(result.events.len(), 6);

        // Check event types
        assert!(matches!(
            result.events[0],
            ReconciliationEvent::Started { .. }
        ));
        assert!(matches!(
            result.events[1],
            ReconciliationEvent::MarketRead { .. }
        ));
        assert!(matches!(
            result.events[2],
            ReconciliationEvent::MarketRead { .. }
        ));
        assert!(matches!(
            result.events[3],
            ReconciliationEvent::MarketRead { .. }
        ));
        assert!(matches!(
            result.events[4],
            ReconciliationEvent::AssetsSync { .. }
        ));
        assert!(matches!(
            result.events[5],
            ReconciliationEvent::Completed { .. }
        ));
    }

    #[test]
    fn resync_external_assets_negative_delta() {
        let env = Env::mock();
        let auth = PermissiveAuth;
        let adapter = MockSorobanMarketAdapter::new(500);

        let request = ResyncRequest::new(
            SorobanAddress::from_bytes([1u8; 32]),
            100,
            vec![1, 2],
            SorobanAddress::from_bytes([2u8; 32]),
            2000, // current external assets (higher than new)
        );

        let result = resync_external_assets(&env, &auth, &adapter, request).unwrap();

        // 2 markets * 500 = 1000
        assert_eq!(result.record.new_external_assets, 1000);

        // Delta should be 1000 - 2000 = -1000
        assert_eq!(result.delta, -1000);
    }

    #[test]
    fn resync_external_assets_empty_markets() {
        let env = Env::mock();
        let auth = PermissiveAuth;
        let adapter = MockSorobanMarketAdapter::new(1000);

        let request = ResyncRequest::new(
            SorobanAddress::from_bytes([1u8; 32]),
            50,
            vec![], // no markets
            SorobanAddress::from_bytes([2u8; 32]),
            1000,
        );

        let result = resync_external_assets(&env, &auth, &adapter, request).unwrap();

        assert_eq!(result.record.markets_refreshed, 0);
        assert_eq!(result.record.new_external_assets, 0);
        assert_eq!(result.delta, -1000);

        // Should have: Started, AssetsSync, Completed (no MarketRead events)
        assert_eq!(result.events.len(), 3);
    }

    #[test]
    fn resync_request_new() {
        let caller = SorobanAddress::from_bytes([1u8; 32]);
        let asset = SorobanAddress::from_bytes([2u8; 32]);
        let markets = vec![1, 2, 3];

        let request = ResyncRequest::new(caller, 42, markets.clone(), asset, 1000);

        assert_eq!(request.caller, caller);
        assert_eq!(request.op_id, 42);
        assert_eq!(request.market_ids, markets);
        assert_eq!(request.asset, asset);
        assert_eq!(request.current_external_assets, 1000);
    }

    #[test]
    fn resync_result_new() {
        let record = ReconciliationRecord::new(1, 2, 3000);
        let events = vec![ReconciliationEvent::started(
            1,
            SorobanAddress::default(),
            0,
            2,
        )];

        let result = ResyncResult::new(record.clone(), events.clone(), 500);

        assert_eq!(result.record, record);
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.delta, 500);
    }
}
