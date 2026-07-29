use std::{fs, process::Command};

use serde_json::{json, Value};

const ACCOUNT: &str = "GBRFSXJNPLMYJV7EBFTBZT2PU6KN5WWPX3UKHDAAQQT7BNS7QTFCS3AY";
const VAULT: &str = "CDY3B7IXFN5L4OY4UFFS2FA4MAQWJZLJD76LW37S7HFVWRS3RPQ2SIXX";

#[test]
fn json_dry_run_keeps_stdout_machine_readable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = dir.path().join("manifest.json");
    fs::write(
        &manifest,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "network": "testnet",
            "artifacts": {},
            "contracts": {
                "vault": {
                    "contract_id": VAULT,
                    "wasm_hash": "predeployed",
                    "constructor_args": {},
                    "initialized": true
                }
            },
            "transactions": []
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");

    let output = Command::new(env!("CARGO_BIN_EXE_tmplr-soroban-vault"))
        .args([
            "--state",
            manifest.to_str().expect("manifest path"),
            "--source-account",
            "alice",
            "--json",
            "--dry-run",
            "user",
            "deposit",
            "--operator",
            ACCOUNT,
            "--assets-raw",
            "1",
        ])
        .output()
        .expect("run CLI");

    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("stdout must be JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["type"], "command");
    let stderr = String::from_utf8(output.stderr).expect("stderr UTF-8");
    assert!(stderr.contains("dry-run:"));
    assert!(stderr.contains("stellar contract invoke"));
}
