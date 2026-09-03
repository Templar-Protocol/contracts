use std::{fs, process::Command};

use serde_json::Value;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tmplr-oft-bridge"))
}

#[test]
fn usdc_refusal_precedes_state_access() {
    let output = binary()
        .args([
            "asset",
            "wrap",
            "--asset",
            "USDC",
            "--asset-kind",
            "usdc",
            "--state",
            "/unreachable/secret-state",
            "--desired",
            "/unreachable/secret-desired.json",
            "--name",
            "Wrapped USDC",
            "--symbol",
            "wUSDC",
            "--execute",
        ])
        .output()
        .expect("run CLI");
    assert_eq!(output.status.code(), Some(3));
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("JSON output");
    assert_eq!(envelope["error"]["code"], "unsupported_use_cctp");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("secret-state"));
}

#[test]
fn init_preview_does_not_write_state() {
    let directory = tempfile::tempdir().expect("tempdir");
    let desired = directory.path().join("desired.json");
    let state = directory.path().join("state");
    fs::write(
        &desired,
        serde_json::json!({
            "schema_name":"desired_route",
            "schema_version":1,
            "route_id":"test-route",
            "identity":{
                "environment":"stellar_testnet_sepolia",
                "stellar_passphrase":"Test SDF Network ; September 2015",
                "stellar_eid":40600,
                "stellar_endpoint":"CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV",
                "stellar_endpoint_code_hash":"01",
                "evm_chain_id":11_155_111,
                "evm_eid":40161,
                "evm_endpoint":"0x6EDCE65403992e310A62460808c4b910D972f10f",
                "evm_endpoint_code_hash":"02"
            },
            "asset":{"kind":"native_sac","asset_id":"native","local_decimals":7},
            "stellar_owner":"GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "stellar_delegate":"GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "evm_owner":"0x0000000000000000000000000000000000000001",
            "evm_delegate":"0x0000000000000000000000000000000000000001"
        })
        .to_string(),
    )
    .expect("write desired");

    let output = binary()
        .args(["init", "--desired"])
        .arg(&desired)
        .arg("--state")
        .arg(&state)
        .output()
        .expect("run CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!state.exists());
}

#[test]
fn mainnet_mutation_is_hard_disabled() {
    let directory = tempfile::tempdir().expect("tempdir");
    let state = directory.path().join("state");
    fs::create_dir(&state).expect("state directory");
    fs::write(
        state.join("route.json"),
        serde_json::json!({
            "schema_name":"route_state","schema_version":1,"route_id":"mainnet",
            "desired_sha256":"00",
            "identity":{
                "environment":"stellar_mainnet_ethereum",
                "stellar_passphrase":"Public Global Stellar Network ; September 2015",
                "stellar_eid":30600,
                "stellar_endpoint":"CCQLLRE5JBAWYCW3KTWOIWLMFDUOKROQVZNSALQMGOSXNW3ERUOWTZGK",
                "stellar_endpoint_code_hash":"01",
                "evm_chain_id":1,"evm_eid":30101,
                "evm_endpoint":"0x0000000000000000000000000000000000000001",
                "evm_endpoint_code_hash":"02"
            },
            "asset":{"kind":"native_sac","asset_id":"native","local_decimals":7},
            "opening_custody":null,"operations_log":"operations.jsonl","messages_log":"messages.jsonl","lock_file":".lock",
            "contracts":{},"effective_config":{}
        })
        .to_string(),
    ).expect("state");
    fs::write(state.join("operations.jsonl"), "").expect("operations");
    fs::write(state.join("messages.jsonl"), "").expect("messages");

    let output = binary()
        .args(["contain", "outbound", "--state"])
        .arg(&state)
        .args([
            "--direction",
            "stellar-to-evm",
            "--proposal-out",
            "proposal.json",
        ])
        .output()
        .expect("run CLI");
    assert_eq!(output.status.code(), Some(3));
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("JSON output");
    assert_eq!(
        envelope["error"]["code"],
        "production_mutation_unsupported_v1"
    );
    assert!(!directory.path().join("proposal.json").exists());
}
