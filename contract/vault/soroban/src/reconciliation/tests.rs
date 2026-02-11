use super::*;
use crate::auth::PermissiveAuth;
use crate::market::TestMarketAdapter;
use alloc::vec;
use core::cell::Cell;
use soroban_sdk::testutils::Address as _;

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
        (1, asset.clone()).into(),
        (2, asset.clone()).into(),
        (3, asset).into(),
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
    let caller: KernelAddress = [1u8; 32];
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
    let env = Env::default();
    let auth = PermissiveAuth;
    let adapter = TestMarketAdapter::new(1000);

    let request = ResyncRequest::new(
        [1u8; 32],
        42,
        vec![1, 2, 3],
        SdkAddress::generate(&env),
        2500, // current external assets
    );

    let result = resync_external_assets(&env, &auth, &adapter, request).unwrap();

    // Should have 3 markets * 1000 = 3000 total
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
    let env = Env::default();
    let auth = PermissiveAuth;
    let adapter = TestMarketAdapter::new(500);

    let request = ResyncRequest::new(
        [1u8; 32],
        100,
        vec![1, 2],
        SdkAddress::generate(&env),
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
    let env = Env::default();
    let auth = PermissiveAuth;
    let adapter = TestMarketAdapter::new(1000);

    let request = ResyncRequest::new(
        [1u8; 32],
        50,
        vec![], // no markets
        SdkAddress::generate(&env),
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
    let env = Env::default();
    let caller: KernelAddress = [1u8; 32];
    let asset = SdkAddress::generate(&env);
    let markets = vec![1, 2, 3];

    let request = ResyncRequest::new(caller, 42, markets.clone(), asset.clone(), 1000);

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
        1, [0u8; 32], // kernel address default
        0, 2,
    )];

    let result = ResyncResult::new(record.clone(), events.clone(), 500);

    assert_eq!(result.record, record);
    assert_eq!(result.events.len(), 1);
    assert_eq!(result.delta, 500);
}
