use serde_json::json;

use super::parse_cli;
use super::GatewayCli;
use crate::gateway_cli::command::command;

#[test]
fn parses_registry_list_versions_typed_args() {
    let cli = parse_cli([
        "tmplrmgr",
        "registry",
        "listVersions",
        "--registry-id",
        "registry.testnet",
        "--offset",
        "3",
        "--limit",
        "9",
    ]);

    assert_eq!(cli.rpc_method(), "registry.listVersions");
    assert_eq!(
        cli.params(),
        &json!({ "registry_id": "registry.testnet", "offset": 3, "limit": 9 })
    );
}

#[test]
fn parses_registry_list_deployments_typed_args() {
    let cli = parse_cli([
        "tmplrmgr",
        "registry",
        "listDeployments",
        "--registry-id",
        "registry.testnet",
        "--offset",
        "10",
        "--limit",
        "25",
    ]);

    assert_eq!(cli.rpc_method(), "registry.listDeployments");
    assert_eq!(
        cli.params(),
        &json!({ "registry_id": "registry.testnet", "offset": 10, "limit": 25 })
    );
}

#[test]
fn parses_registry_get_deployment_typed_args() {
    let cli = parse_cli([
        "tmplrmgr",
        "registry",
        "getDeployment",
        "--registry-id",
        "registry.testnet",
        "--account-id",
        "market.testnet",
    ]);

    assert_eq!(cli.rpc_method(), "registry.getDeployment");
    assert_eq!(
        cli.params(),
        &json!({ "registry_id": "registry.testnet", "account_id": "market.testnet" })
    );
}

#[test]
fn parses_registry_list_deployments_by_kind_typed_args() {
    let cli = parse_cli([
        "tmplrmgr",
        "registry",
        "listDeploymentsByKind",
        "--registry-id",
        "registry.testnet",
        "--kind",
        "market",
        "--offset",
        "2",
        "--limit",
        "4",
    ]);

    assert_eq!(cli.rpc_method(), "registry.listDeploymentsByKind");
    assert_eq!(
        cli.params(),
        &json!({ "registry_id": "registry.testnet", "kind": "market", "offset": 2, "limit": 4 })
    );
}

#[test]
fn parses_registry_kebab_case_aliases() {
    let cases = [
        ("list-versions", "registry.listVersions"),
        ("list-deployments", "registry.listDeployments"),
        ("list-deployments-by-kind", "registry.listDeploymentsByKind"),
        ("get-deployment", "registry.getDeployment"),
    ];

    for (method, rpc_method) in cases {
        let mut args = vec![
            "tmplrmgr",
            "registry",
            method,
            "--registry-id",
            "registry.testnet",
        ];
        if method == "get-deployment" {
            args.extend(["--account-id", "market.testnet"]);
        }
        if method == "list-deployments-by-kind" {
            args.extend(["--kind", "market"]);
        }

        let cli = parse_cli(args);

        assert_eq!(cli.rpc_method(), rpc_method);
    }
}

#[test]
fn json_params_take_precedence_over_typed_args() {
    let cli = parse_cli([
        "tmplrmgr",
        "registry",
        "listVersions",
        "--registry-id",
        "typed.testnet",
        "--json",
        r#"{"registry_id":"json.testnet","offset":7}"#,
    ]);

    assert_eq!(
        cli.params(),
        &json!({ "registry_id": "json.testnet", "offset": 7 })
    );
}

#[test]
fn json_file_params_take_precedence_over_typed_args() {
    let path = std::env::temp_dir().join(format!(
        "tmplrmgr-gateway-cli-{}-{}.json",
        std::process::id(),
        "json-file-precedence"
    ));
    std::fs::write(&path, r#"{"registry_id":"file.testnet","limit":5}"#)
        .expect("write gateway params fixture");

    let matches = command()
        .try_get_matches_from([
            "tmplrmgr",
            "registry",
            "listVersions",
            "--registry-id",
            "typed.testnet",
            "--json-file",
            path.to_str().expect("fixture path should be valid unicode"),
        ])
        .expect("typed gateway command with json-file should parse");
    let cli = GatewayCli::from_matches(&matches).expect("json-file params should load");
    std::fs::remove_file(&path).expect("remove gateway params fixture");

    assert_eq!(
        cli.params(),
        &json!({ "registry_id": "file.testnet", "limit": 5 })
    );
}

#[test]
fn typed_registry_method_rejects_missing_required_field_without_json() {
    let error = command()
        .try_get_matches_from(["tmplrmgr", "registry", "listVersions"])
        .expect_err("typed registry command should require registry-id without JSON params");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}
