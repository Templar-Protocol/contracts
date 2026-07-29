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

#[test]
fn command_error_envelope_preserves_the_full_error_chain() {
    let cli = base_cli("manifest.json".into(), Commands::Status);
    let error = anyhow::anyhow!("stellar rejected the transaction").context("preflight failed");
    let value = serde_json::to_value(OutputEnvelope::error(&cli, &error)).expect("error envelope");

    assert_eq!(
        value["error"]["message"],
        "preflight failed: stellar rejected the transaction"
    );
}

#[test]
fn command_envelope_reports_only_the_labeled_transaction_hash() {
    let transaction_hash = "54b30bb33c391796bad397cff54f3426684f45e2b3c884ac2fbefa22e4deb92b";
    let response = Response::Command {
        stdout: "\"1000000\"".to_string(),
        stderr: format!(
            "Signing transaction: {transaction_hash}\nhttps://stellar.expert/explorer/testnet/tx/{transaction_hash}\nowner bytes: 611ff11d17c7bdec6c36f8da7483fa6d6a0236607634f4f442484c14333aff10"
        ),
    };

    assert_eq!(response.tx_hashes(), vec![transaction_hash]);
}
