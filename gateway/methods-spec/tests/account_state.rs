use templar_gateway_methods_spec::account::{GetCode, ViewState};
use templar_gateway_types::Base64Bytes;

#[test]
fn account_state_reads_have_stable_wire_shapes() {
    let code = GetCode {
        account_id: "target.near".parse().unwrap(),
    };
    let expected: serde_json::Value =
        serde_json::from_str(r#"{"account_id":"target.near"}"#).unwrap();
    assert_eq!(serde_json::to_value(code).unwrap(), expected);

    let state = ViewState {
        account_id: "target.near".parse().unwrap(),
        prefix: Base64Bytes(b"prefix".to_vec()),
    };
    let expected: serde_json::Value =
        serde_json::from_str(r#"{"account_id":"target.near","prefix":"cHJlZml4"}"#).unwrap();
    assert_eq!(serde_json::to_value(state).unwrap(), expected);
}

#[test]
fn account_state_results_use_base64_wire_values() {
    use templar_gateway_methods_spec::account::{GetCodeResult, StateEntry, ViewStateResult};

    assert_eq!(
        serde_json::to_value(GetCodeResult {
            code: Base64Bytes(b"wasm".to_vec()),
        })
        .unwrap(),
        serde_json::from_str::<serde_json::Value>(r#"{"code":"d2FzbQ=="}"#).unwrap()
    );
    assert_eq!(
        serde_json::to_value(ViewStateResult {
            values: vec![StateEntry {
                key: Base64Bytes(b"k".to_vec()),
                value: Base64Bytes(b"v".to_vec()),
            }],
        })
        .unwrap(),
        serde_json::from_str::<serde_json::Value>(r#"{"values":[{"key":"aw==","value":"dg=="}]}"#,)
            .unwrap()
    );
}

#[test]
fn protocol_limits_request_is_empty() {
    use templar_gateway_methods_spec::chain::GetProtocolLimits;

    assert_eq!(
        serde_json::to_value(GetProtocolLimits).unwrap(),
        serde_json::Value::Null
    );
}
#[test]
fn account_state_schema_describes_binary_prefix() {
    let schema = serde_json::to_value(schemars::schema_for!(ViewState)).unwrap();
    assert_eq!(
        schema["properties"]["prefix"]["$ref"],
        "#/definitions/Base64Bytes"
    );
}
