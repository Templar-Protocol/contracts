use std::fs;

use std::collections::BTreeMap;
use templar_oft_bridge_cli::{
    domain::{AssetKind, AssetPolicyV1, ChainIdentityV1, DesiredRouteV1, Environment},
    state::{OperationEventV1, OperationState, RouteStore},
};

fn desired() -> DesiredRouteV1 {
    DesiredRouteV1 {
        schema_name: "desired_route".into(),
        schema_version: 1,
        route_id: "route-a".into(),
        identity: ChainIdentityV1 {
            environment: Environment::StellarTestnetSepolia,
            stellar_passphrase: "Test SDF Network ; September 2015".into(),
            stellar_eid: 40_600,
            stellar_endpoint: "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV".into(),
            stellar_endpoint_code_hash: "01".into(),
            evm_chain_id: 11_155_111,
            evm_eid: 40_161,
            evm_endpoint: "0x6EDCE65403992e310A62460808c4b910D972f10f".into(),
            evm_endpoint_code_hash: "02".into(),
        },
        asset: AssetPolicyV1 {
            kind: AssetKind::NativeSac,
            asset_id: "native".into(),
            local_decimals: 7,
            issuer_custodian_evidence_sha256: None,
            destination_acceptance_evidence_sha256: None,
            custody_risk_acceptance_sha256: None,
            forbidden_classic_issuer: None,
            evidence: BTreeMap::default(),
        },
        stellar_owner: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
        stellar_delegate: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
        evm_owner: "0x0000000000000000000000000000000000000001".into(),
        evm_delegate: "0x0000000000000000000000000000000000000001".into(),
        config: BTreeMap::default(),
    }
}

#[test]
fn creates_route_and_verifies_hash_chain() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create route");
    let _lock = store.lock().expect("lock");
    store
        .append_operation(
            OperationEventV1 {
                operation_id: "op-1".into(),
                state: OperationState::Planned,
                detail: serde_json::json!({"x":1}),
            },
            None,
        )
        .expect("append");
    let records = store
        .verify_log::<OperationEventV1>(std::path::Path::new("operations.jsonl"), "operations")
        .expect("verify");
    assert_eq!(records.len(), 1);
}

#[test]
fn detects_log_tampering() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create route");
    store
        .append_operation(
            OperationEventV1 {
                operation_id: "op-1".into(),
                state: OperationState::Planned,
                detail: serde_json::json!({"x":1}),
            },
            None,
        )
        .expect("append");
    let path = root.join("operations.jsonl");
    let raw = fs::read_to_string(&path)
        .expect("read")
        .replace("op-1", "op-2");
    fs::write(path, raw).expect("tamper");
    let error = store
        .verify_log::<OperationEventV1>(std::path::Path::new("operations.jsonl"), "operations")
        .expect_err("must reject tamper");
    assert!(error.to_string().contains("digest mismatch"));
}

#[test]
fn route_lock_excludes_second_writer() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create route");
    let first = store.lock().expect("first lock");
    let error = store.lock().expect_err("second lock must fail");
    assert!(error.to_string().contains("busy"));
    drop(first);
    store.lock().expect("lock after release");
}
