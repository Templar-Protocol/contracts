//! Monotonic canary and packet message state machine tests.

use std::collections::BTreeMap;

use templar_oft_bridge_cli::canary::{CanaryMessageV1, CanaryStage};
use templar_oft_bridge_cli::error::Error;

#[test]
fn arm_starts_in_armed_stage() {
    let message = CanaryMessageV1::arm("guid-canary-1").unwrap();
    assert_eq!(message.guid(), "guid-canary-1");
    assert_eq!(message.stage(), CanaryStage::Armed);
    assert_eq!(message.ledger_sequence(), None);
    assert!(!message.can_recover());
    assert_eq!(message.obligations(), &BTreeMap::new());
}

#[test]
fn empty_guid_is_refused() {
    let error = CanaryMessageV1::arm("  ").unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)));
}

#[test]
fn full_forward_path_is_monotonic() {
    let mut message = CanaryMessageV1::arm("guid-path").unwrap();
    message.advance(CanaryStage::Dispatched, Some(100)).unwrap();
    message.advance(CanaryStage::Delivered, Some(120)).unwrap();
    message
        .advance(CanaryStage::Acknowledged, Some(121))
        .unwrap();
    message.advance(CanaryStage::Settled, None).unwrap();
    assert_eq!(message.stage(), CanaryStage::Settled);
    assert_eq!(message.ledger_sequence(), Some(121));
    assert!(!message.can_recover());
}

#[test]
fn backward_and_equal_transitions_are_conflicts() {
    let mut message = CanaryMessageV1::arm("guid-back").unwrap();
    message.advance(CanaryStage::Dispatched, None).unwrap();
    let backward = message.advance(CanaryStage::Armed, None).unwrap_err();
    assert!(matches!(backward, Error::Conflict(_)));
    let equal = message.advance(CanaryStage::Dispatched, None).unwrap_err();
    assert!(matches!(equal, Error::Conflict(_)));
}

#[test]
fn forward_jumps_are_allowed() {
    let mut message = CanaryMessageV1::arm("guid-jump").unwrap();
    message.advance(CanaryStage::Acknowledged, None).unwrap();
    assert_eq!(message.stage(), CanaryStage::Acknowledged);
}

#[test]
fn ledger_sequences_are_monotonic() {
    let mut message = CanaryMessageV1::arm("guid-ledger").unwrap();
    message.advance(CanaryStage::Dispatched, Some(50)).unwrap();
    let below = message
        .advance(CanaryStage::Delivered, Some(49))
        .unwrap_err();
    assert!(matches!(below, Error::Conflict(_)));
    message.advance(CanaryStage::Delivered, Some(51)).unwrap();
    assert_eq!(message.ledger_sequence(), Some(51));
}

#[test]
fn recovery_window_covers_moving_stages_only() {
    let mut message = CanaryMessageV1::arm("guid-recover").unwrap();
    assert!(!message.can_recover());
    message.advance(CanaryStage::Dispatched, None).unwrap();
    assert!(message.can_recover());
    message.advance(CanaryStage::Delivered, None).unwrap();
    assert!(message.can_recover());
    message.advance(CanaryStage::Acknowledged, None).unwrap();
    assert!(message.can_recover());
    message.advance(CanaryStage::Settled, None).unwrap();
    assert!(!message.can_recover());
}

#[test]
fn obligations_record_once_then_refuse_duplicates() {
    let mut message = CanaryMessageV1::arm("guid-obligation").unwrap();
    message
        .set_obligation("refund_destination", "GA refund", false)
        .unwrap();
    assert_eq!(message.obligation("refund_destination"), Some("GA refund"));
    let duplicate = message
        .set_obligation("refund_destination", "GA other", false)
        .unwrap_err();
    assert!(matches!(duplicate, Error::Conflict(_)));
    assert_eq!(message.obligation("refund_destination"), Some("GA refund"));
}

#[test]
fn obligations_override_when_explicitly_allowed() {
    let mut message = CanaryMessageV1::arm("guid-override").unwrap();
    message.set_obligation("memo", "first", false).unwrap();
    message.set_obligation("memo", "second", true).unwrap();
    assert_eq!(message.obligation("memo"), Some("second"));
}

#[test]
fn empty_obligation_keys_and_values_are_refused() {
    let mut message = CanaryMessageV1::arm("guard").unwrap();
    let empty_key = message.set_obligation("  ", "v", false).unwrap_err();
    assert!(matches!(empty_key, Error::InvalidInput(_)));
    let empty_value = message.set_obligation("k", " ", false).unwrap_err();
    assert!(matches!(empty_value, Error::InvalidInput(_)));
}

#[test]
fn stage_labels_round_trip_and_reject_unknowns() {
    for stage in [
        CanaryStage::Armed,
        CanaryStage::Dispatched,
        CanaryStage::Delivered,
        CanaryStage::Acknowledged,
        CanaryStage::Settled,
    ] {
        assert_eq!(CanaryStage::parse(stage.label()).unwrap(), stage);
    }
    let unknown = CanaryStage::parse("replayed").unwrap_err();
    assert!(matches!(unknown, Error::InvalidInput(_)));
}

#[test]
fn stages_rank_monotonically() {
    let ranked: Vec<u8> = [
        CanaryStage::Armed,
        CanaryStage::Dispatched,
        CanaryStage::Delivered,
        CanaryStage::Acknowledged,
        CanaryStage::Settled,
    ]
    .iter()
    .copied()
    .map(CanaryStage::rank)
    .collect();
    let mut sorted = ranked.clone();
    sorted.sort_unstable();
    assert_eq!(ranked, sorted);
}
