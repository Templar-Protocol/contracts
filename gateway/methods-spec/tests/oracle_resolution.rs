#![allow(
    clippy::unwrap_used,
    reason = "Malformed wire fixtures or unexpected schema shapes must fail these contract tests immediately"
)]

use templar_gateway_methods_spec::oracle::{ResolvePrice, ResolvePriceAllSources};

#[test]
fn all_sources_resolution_reuses_the_canonical_flattened_wire_shape() {
    let canonical = ResolvePrice {
        oracle_id: "oracle.near".parse().unwrap(),
        price_id: templar_common::oracle::pyth::PriceIdentifier([7; 32]),
        age: 60,
        pyth: vec![],
        redstone: vec![],
        lazer: vec![],
    };
    let expected = serde_json::json!({
        "oracle_id": "oracle.near",
        "price_id": "07".repeat(32),
        "age": 60,
        "pyth": [],
        "redstone": [],
        "lazer": [],
    });
    let strict = ResolvePriceAllSources {
        request: canonical.clone(),
    };
    assert_eq!(serde_json::to_value(&canonical).unwrap(), expected);
    assert_eq!(serde_json::to_value(&strict).unwrap(), expected);
    assert_eq!(
        serde_json::from_value::<ResolvePriceAllSources>(expected.clone()).unwrap(),
        strict
    );
    let mut without_lazer = expected;
    without_lazer.as_object_mut().unwrap().remove("lazer");
    assert_eq!(
        serde_json::from_value::<ResolvePriceAllSources>(without_lazer).unwrap(),
        strict
    );
}

#[test]
fn all_sources_resolution_schema_exposes_flattened_request_fields() {
    let schema = serde_json::to_value(schemars::schema_for!(ResolvePriceAllSources)).unwrap();
    let properties = schema["properties"].as_object().unwrap();
    assert!(!properties.contains_key("request"));
    for field in ["oracle_id", "price_id", "age", "pyth", "redstone", "lazer"] {
        assert!(properties.contains_key(field), "missing {field}");
    }
}
