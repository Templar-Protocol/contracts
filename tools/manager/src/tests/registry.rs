use clap::Parser;
use serde_json::json;

use crate::cli::{Cli, Command};
use crate::commands::registry::RegistryNs;

#[test]
fn parses_registry_list_versions_typed_args() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "list-versions",
        "--registry-id",
        "registry.testnet",
        "--offset",
        "3",
        "--limit",
        "9",
    ])
    .expect("list-versions should parse");

    let params = match cli.command {
        Command::Registry {
            command: RegistryNs::ListVersions(cmd),
        } => cmd.parse(),
        _ => panic!("expected Registry::ListVersions"),
    };

    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "registry_id": "registry.testnet", "offset": 3, "count": 9 })
    );
}

#[test]
fn parses_registry_list_deployments_typed_args() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "list-deployments",
        "--registry-id",
        "registry.testnet",
        "--offset",
        "10",
        "--limit",
        "25",
    ])
    .expect("list-deployments should parse");

    let params = match cli.command {
        Command::Registry {
            command: RegistryNs::ListDeployments(cmd),
        } => cmd.parse(),
        _ => panic!("expected Registry::ListDeployments"),
    };

    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "registry_id": "registry.testnet", "offset": 10, "count": 25 })
    );
}

#[test]
fn parses_registry_get_deployment_typed_args() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "get-deployment",
        "--registry-id",
        "registry.testnet",
        "--account-id",
        "market.testnet",
    ])
    .expect("get-deployment should parse");

    let params = match cli.command {
        Command::Registry {
            command: RegistryNs::GetDeployment(cmd),
        } => cmd.parse(),
        _ => panic!("expected Registry::GetDeployment"),
    };

    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "registry_id": "registry.testnet", "account_id": "market.testnet" })
    );
}

#[test]
fn parses_registry_list_deployments_by_kind_typed_args() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "list-deployments-by-kind",
        "--registry-id",
        "registry.testnet",
        "--kind",
        "market",
        "--offset",
        "2",
        "--limit",
        "4",
    ])
    .expect("list-deployments-by-kind should parse");

    let params = match cli.command {
        Command::Registry {
            command: RegistryNs::ListDeploymentsByKind(cmd),
        } => cmd.parse(),
        _ => panic!("expected Registry::ListDeploymentsByKind"),
    };

    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "registry_id": "registry.testnet", "kind": "market", "offset": 2, "count": 4 })
    );
}

#[test]
fn parses_registry_kebab_case_aliases() {
    let cases = [
        ("list-versions", "listVersions"),
        ("list-deployments", "listDeployments"),
        ("list-deployments-by-kind", "listDeploymentsByKind"),
        ("get-deployment", "getDeployment"),
    ];

    for (kebab, _camel) in cases {
        let mut args = vec![
            "tmplrmgr",
            "registry",
            kebab,
            "--registry-id",
            "registry.testnet",
        ];
        if kebab == "get-deployment" {
            args.extend(["--account-id", "market.testnet"]);
        }
        if kebab == "list-deployments-by-kind" {
            args.extend(["--kind", "market"]);
        }

        let cli = Cli::try_parse_from(&args).expect("kebab-case command should parse");

        match cli.command {
            Command::Registry { .. } => {}
            _ => panic!("expected Registry variant for {kebab}"),
        }
    }
}

#[test]
fn typed_registry_method_rejects_missing_required_field() {
    let error = Cli::try_parse_from(["tmplrmgr", "registry", "list-versions"])
        .expect_err("list-versions should require registry-id");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}
