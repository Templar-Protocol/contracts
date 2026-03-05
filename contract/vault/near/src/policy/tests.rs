use super::{SupplyQueue, WithdrawRoute};
use templar_common::vault::MarketId;

#[test]
fn supply_queue_roundtrips_vec_layout() {
    let source = vec![MarketId(1), MarketId(2)];
    let queue = SupplyQueue::from(source.clone());
    let roundtrip: Vec<MarketId> = queue.into();
    assert_eq!(roundtrip, source);
}

#[test]
fn withdraw_route_roundtrips_vec_layout() {
    let source = vec![MarketId(3), MarketId(4)];
    let route = WithdrawRoute::from(source.clone());
    let roundtrip: Vec<MarketId> = route.into();
    assert_eq!(roundtrip, source);
}
