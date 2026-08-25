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
fn account_state_schema_describes_binary_prefix() {
    let schema = serde_json::to_value(schemars::schema_for!(ViewState)).unwrap();
    assert_eq!(
        schema["properties"]["prefix"]["$ref"],
        "#/definitions/Base64Bytes"
    );
}
