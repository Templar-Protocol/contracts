use serde_json::json;

use super::parse_cli;
use super::GatewayCli;
use crate::gateway_cli::command::command;

#[test]
fn parses_storage_typed_args() {
    macro_rules! case {
        ($args:expr, $rpc_method:expr, $params:expr) => {{
            let cli = parse_cli($args);
            assert_eq!(cli.rpc_method(), $rpc_method);
            assert_eq!(cli.params(), &$params);
        }};
    }

    case!(
        [
            "tmplrmgr",
            "storage",
            "getBalanceBounds",
            "--contract-id",
            "storage.testnet"
        ],
        "storage.getBalanceBounds",
        json!({ "contract_id": "storage.testnet" })
    );
    case!(
        [
            "tmplrmgr",
            "storage",
            "getBalanceOf",
            "--contract-id",
            "storage.testnet",
            "--account-id",
            "alice.testnet"
        ],
        "storage.getBalanceOf",
        json!({ "contract_id": "storage.testnet", "account_id": "alice.testnet" })
    );
    case!(
        [
            "tmplrmgr",
            "storage",
            "deposit",
            "--contract-id",
            "storage.testnet",
            "--beneficiary-id",
            "beneficiary.testnet",
            "--registration-only",
            "--deposit",
            "1250000000000000000000000"
        ],
        "storage.deposit",
        json!({ "contract_id": "storage.testnet", "beneficiary_id": "beneficiary.testnet", "registration_only": true, "deposit": "1250000000000000000000000" })
    );
    case!(
        [
            "tmplrmgr",
            "storage",
            "unregister",
            "--contract-id",
            "storage.testnet",
            "--force"
        ],
        "storage.unregister",
        json!({ "contract_id": "storage.testnet", "force": true })
    );
    case!(
        [
            "tmplrmgr",
            "storage",
            "ensureDeposit",
            "--contract-id",
            "storage.testnet",
            "--account-id",
            "alice.testnet",
            "--mode",
            "minimum_total",
            "--amount",
            "2500000000000000000000000"
        ],
        "storage.ensureDeposit",
        json!({ "contract_id": "storage.testnet", "account_id": "alice.testnet", "mode": { "mode": "minimum_total", "amount": "2500000000000000000000000" } })
    );
}

#[test]
fn parses_storage_kebab_case_aliases() {
    macro_rules! case {
        ($args:expr, $rpc_method:expr) => {{
            let cli = parse_cli($args);
            assert_eq!(cli.rpc_method(), $rpc_method);
        }};
    }

    case!(
        [
            "tmplrmgr",
            "storage",
            "get-balance-bounds",
            "--contract-id",
            "storage.testnet"
        ],
        "storage.getBalanceBounds"
    );
    case!(
        [
            "tmplrmgr",
            "storage",
            "get-balance-of",
            "--contract-id",
            "storage.testnet",
            "--account-id",
            "alice.testnet"
        ],
        "storage.getBalanceOf"
    );
    case!(
        [
            "tmplrmgr",
            "storage",
            "ensure-deposit",
            "--contract-id",
            "storage.testnet",
            "--account-id",
            "alice.testnet",
            "--mode",
            "registered"
        ],
        "storage.ensureDeposit"
    );
}

#[test]
fn json_params_take_precedence_over_storage_typed_args() {
    let cli = parse_cli([
        "tmplrmgr",
        "storage",
        "deposit",
        "--contract-id",
        "typed.testnet",
        "--beneficiary-id",
        "typed-beneficiary.testnet",
        "--registration-only",
        "--deposit",
        "1",
        "--json",
        r#"{"contract_id":"json.testnet","beneficiary_id":"json-beneficiary.testnet","registration_only":false,"deposit":"3500000000000000000000000"}"#,
    ]);

    assert_eq!(
        cli.params(),
        &json!({ "contract_id": "json.testnet", "beneficiary_id": "json-beneficiary.testnet", "registration_only": false, "deposit": "3500000000000000000000000" })
    );
}

#[test]
fn registered_ensure_deposit_rejects_amount() {
    let matches = command()
        .try_get_matches_from([
            "tmplrmgr",
            "storage",
            "ensureDeposit",
            "--contract-id",
            "storage.testnet",
            "--account-id",
            "alice.testnet",
            "--mode",
            "registered",
            "--amount",
            "1",
        ])
        .expect("registered ensureDeposit command should parse before typed validation");

    let error = match GatewayCli::from_matches(&matches) {
        Ok(_) => panic!("registered ensureDeposit should reject --amount"),
        Err(error) => error,
    };

    assert!(error
        .to_string()
        .contains("--amount is only valid for minimum_total or minimum_available mode"));
}
