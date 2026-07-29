use super::*;

#[test]
fn json_envelope_has_stable_machine_fields() {
    let cli = base_cli("manifest.json".into(), Commands::Status);
    let response = Response::message("ok".to_string());
    let value =
        serde_json::to_value(OutputEnvelope::success(&cli, &response)).expect("json envelope");

    assert_eq!(value["type"], "message");
    assert_eq!(value["ok"], true);
    assert_eq!(value["network"], "testnet");
    assert_eq!(value["manifest"], "manifest.json");
    assert!(value["commands"].is_array());
    assert!(value["tx_hashes"].is_array());
    assert!(value["warnings"].is_array());
    assert_eq!(value["data"]["type"], "message");
}

#[test]
fn parse_error_envelope_reports_secret_argv_code() {
    let error = clap::Error::raw(
        clap::error::ErrorKind::ValueValidation,
        "do not pass secret keys via --source-account",
    );
    let value = serde_json::to_value(ParseErrorEnvelope::new(
        &[
            "tmplr-soroban-vault".to_string(),
            "--json".to_string(),
            "--network".to_string(),
            "testnet".to_string(),
            "status".to_string(),
        ],
        &error,
    ))
    .expect("json envelope");

    assert_eq!(value["type"], "error");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "secret_in_argv");
}
