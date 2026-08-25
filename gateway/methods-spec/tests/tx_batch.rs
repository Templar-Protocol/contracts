//! The `tx.batch` wire shape: renaming or re-tagging a variant breaks every
//! batch already authored against it.

// `allow-expect-in-tests` only covers `#[test]` functions, not the shared helper.
#![allow(clippy::expect_used)]

use serde_json::json;
use templar_gateway_methods_spec::tx::Batch;
use templar_gateway_types::{
    common::ContractArgs, protocol::MAX_ACTIONS_PER_RECEIPT, ActionInput, Base64Bytes,
    ContractMethodName, GlobalContractIdentifierInput, NearGas, NearToken,
};

fn round_trip(action: &ActionInput, expected: serde_json::Value) {
    assert_eq!(
        serde_json::to_value(action).expect("serialize"),
        expected,
        "serialized shape changed"
    );
    assert_eq!(
        &serde_json::from_value::<ActionInput>(expected).expect("deserialize"),
        action,
        "did not survive a round trip"
    );
}

#[test]
fn function_call_matches_the_shape_of_tx_function_call() {
    round_trip(
        &ActionInput::FunctionCall {
            method_name: ContractMethodName("migrate".to_owned()),
            args: ContractArgs::Json(json!({ "from_version": "v0" })),
            gas: NearGas::from_tgas(30),
            deposit: NearToken::from_yoctonear(0),
        },
        json!({
            "action": "function_call",
            "method_name": "migrate",
            "args": { "encoding": "json", "value": { "from_version": "v0" } },
            "gas": "30000000000000",
            "deposit": "0",
        }),
    );
}

#[test]
fn function_call_carries_raw_args() {
    round_trip(
        &ActionInput::FunctionCall {
            method_name: ContractMethodName("patch".to_owned()),
            args: ContractArgs::Raw(Base64Bytes(vec![1, 2, 3])),
            gas: NearGas::from_tgas(100),
            deposit: NearToken::from_yoctonear(1),
        },
        json!({
            "action": "function_call",
            "method_name": "patch",
            "args": { "encoding": "raw", "value": "AQID" },
            "gas": "100000000000000",
            "deposit": "1",
        }),
    );
}

#[test]
fn transfer_round_trips() {
    round_trip(
        &ActionInput::Transfer {
            deposit: NearToken::from_near(2),
        },
        json!({ "action": "transfer", "deposit": "2000000000000000000000000" }),
    );
}

#[test]
fn deploy_contract_carries_base64_code() {
    round_trip(
        &ActionInput::DeployContract {
            code: Base64Bytes(vec![0, 97, 115, 109]),
        },
        json!({ "action": "deploy_contract", "code": "AGFzbQ==" }),
    );
}

#[test]
fn use_global_contract_round_trips_by_account() {
    round_trip(
        &ActionInput::UseGlobalContract {
            contract_identifier: GlobalContractIdentifierInput::AccountId(
                "code.near".parse().expect("valid account id"),
            ),
        },
        json!({
            "action": "use_global_contract",
            "contract_identifier": { "kind": "account_id", "value": "code.near" },
        }),
    );
}

/// JSON-driven: this crate cannot build a `CryptoHash`, and the wire form is the point.
#[test]
fn use_global_contract_round_trips_by_hash() {
    let expected = json!({
        "action": "use_global_contract",
        "contract_identifier": {
            "kind": "code_hash",
            "value": "11111111111111111111111111111111",
        },
    });

    let action: ActionInput = serde_json::from_value(expected.clone()).expect("deserialize");
    assert!(
        matches!(
            action,
            ActionInput::UseGlobalContract {
                contract_identifier: GlobalContractIdentifierInput::CodeHash(_)
            }
        ),
        "a base58 hash must decode to the code-hash variant, got {action:?}"
    );
    assert_eq!(serde_json::to_value(&action).expect("serialize"), expected);
}

#[test]
fn batch_round_trips_with_its_actions_in_order() {
    let batch = Batch {
        receiver_id: "target.near".parse().expect("valid account id"),
        actions: vec![
            ActionInput::DeployContract {
                code: Base64Bytes(vec![0, 97, 115, 109]),
            },
            ActionInput::Transfer {
                deposit: NearToken::from_yoctonear(1),
            },
        ],
    };

    let encoded = serde_json::to_value(&batch).expect("serialize");
    assert_eq!(
        encoded,
        json!({
            "receiver_id": "target.near",
            "actions": [
                { "action": "deploy_contract", "code": "AGFzbQ==" },
                { "action": "transfer", "deposit": "1" },
            ],
        }),
    );
    assert_eq!(
        serde_json::from_value::<Batch>(encoded).expect("deserialize"),
        batch
    );
}

/// A schemars attribute takes a literal, so only this ties it to the constant.
#[test]
fn batch_schema_states_the_planner_s_bounds() {
    let schema = serde_json::to_value(schemars::schema_for!(Batch)).expect("schema");
    let actions = &schema["properties"]["actions"];

    assert_eq!(
        actions["minItems"],
        serde_json::json!(1),
        "schema must reject an empty batch, as the planner does"
    );
    assert_eq!(
        actions["maxItems"],
        serde_json::json!(MAX_ACTIONS_PER_RECEIPT),
        "schema bound drifted from the protocol limit the planner enforces"
    );
}
