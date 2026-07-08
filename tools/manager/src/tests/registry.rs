use std::io::Write as _;

use clap::Parser;
use serde_json::json;
use templar_common::registry::DeployMode;

use crate::cli::{Cli, Command};
use crate::commands::registry::RegistryNs;

/// Write `bytes` to a uniquely-named temp file so `--wasm` tests exercise the
/// real file read without invoking a contract build.
fn temp_wasm(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("tmplrmgr-add-version-{tag}.wasm"));
    let mut file = std::fs::File::create(&path).expect("create temp wasm");
    file.write_all(bytes).expect("write temp wasm");
    path
}

fn add_version_spec(
    args: &[&str],
) -> anyhow::Result<templar_gateway_methods_spec::registry::AddVersion> {
    match Cli::try_parse_from(args)
        .expect("add-version should parse")
        .command
    {
        Command::Registry {
            command: RegistryNs::AddVersion(cmd),
        } => cmd.into_spec(),
        _ => panic!("expected Registry::AddVersion"),
    }
}

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

#[test]
fn add_version_wasm_path_builds_global_hash_spec() {
    let wasm = b"\0asm\x01\0\0\0global";
    let path = temp_wasm("global", wasm);
    let spec = add_version_spec(&[
        "tmplrmgr",
        "registry",
        "add-version",
        "--registry-id",
        "registry.testnet",
        "--wasm",
        path.to_str().unwrap(),
        "--version-key",
        "templar-market-contract@1.4.0#deadbeef",
        "--deploy-mode",
        "global-hash",
    ])
    .expect("into_spec should succeed");
    std::fs::remove_file(&path).ok();

    assert_eq!(spec.registry_id.as_str(), "registry.testnet");
    assert_eq!(spec.version_key, "templar-market-contract@1.4.0#deadbeef");
    assert_eq!(spec.deploy_mode, DeployMode::GlobalHash);
    assert_eq!(spec.code.0, wasm);
    // GlobalHash stakes storage for the code: 1e19 yocto/byte * len * 10.
    let expected = 10_000_000_000_000_000_000u128 * (wasm.len() as u128 * 10);
    assert_eq!(spec.deposit.as_yoctonear(), expected);
}

#[test]
fn add_version_defaults_to_normal_mode_and_minimal_deposit() {
    let path = temp_wasm("normal", b"\0asm-normal");
    let spec = add_version_spec(&[
        "tmplrmgr",
        "registry",
        "add-version",
        "--registry-id",
        "registry.testnet",
        "--wasm",
        path.to_str().unwrap(),
        "--version-key",
        "custom@1.0.0#abc",
    ])
    .expect("into_spec should succeed");
    std::fs::remove_file(&path).ok();

    assert_eq!(spec.deploy_mode, DeployMode::Normal);
    assert_eq!(spec.deposit.as_yoctonear(), 1);
}

#[test]
fn add_version_deposit_override_wins_over_estimate() {
    let path = temp_wasm("override", b"\0asm-override");
    let spec = add_version_spec(&[
        "tmplrmgr",
        "registry",
        "add-version",
        "--registry-id",
        "registry.testnet",
        "--wasm",
        path.to_str().unwrap(),
        "--version-key",
        "custom@1.0.0#abc",
        "--deploy-mode",
        "global-hash",
        "--deposit",
        "3 NEAR",
    ])
    .expect("into_spec should succeed");
    std::fs::remove_file(&path).ok();

    assert_eq!(
        spec.deposit.as_yoctonear(),
        3_000_000_000_000_000_000_000_000
    );
}

#[test]
fn add_version_wasm_requires_explicit_version_key() {
    let path = temp_wasm("no-key", b"\0asm");
    let result = add_version_spec(&[
        "tmplrmgr",
        "registry",
        "add-version",
        "--registry-id",
        "registry.testnet",
        "--wasm",
        path.to_str().unwrap(),
    ]);
    std::fs::remove_file(&path).ok();

    let error = result.expect_err("--wasm without --version-key must fail");
    assert!(
        error.to_string().contains("--version-key"),
        "unexpected error: {error}"
    );
}

#[test]
fn add_version_rejects_conflicting_contract_sources() {
    let error = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "add-version",
        "--registry-id",
        "registry.testnet",
        "--market",
        "--package",
        "proxy-oracle",
    ])
    .expect_err("conflicting contract selectors must be rejected");

    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn add_version_requires_a_contract_source() {
    let error = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "add-version",
        "--registry-id",
        "registry.testnet",
    ])
    .expect_err("a contract source is required");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}

#[test]
fn remove_version_single_builds_spec() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "remove-version",
        "--registry-id",
        "registry.testnet",
        "--version-key",
        "market@1",
    ])
    .expect("remove-version should parse");
    let Command::Registry {
        command: RegistryNs::RemoveVersion(cmd),
    } = cli.command
    else {
        panic!("expected remove-version");
    };
    let spec = cmd.single().expect("single version spec");
    assert_eq!(
        serde_json::to_value(&spec).unwrap()["version_key"],
        "market@1"
    );
}

#[test]
fn remove_version_all_has_no_single_spec() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "remove-version",
        "--registry-id",
        "registry.testnet",
        "--all",
    ])
    .expect("remove-version --all should parse");
    let Command::Registry {
        command: RegistryNs::RemoveVersion(cmd),
    } = cli.command
    else {
        panic!("expected remove-version");
    };
    assert!(cmd.single().is_none());
}

#[test]
fn remove_version_requires_version_key_or_all() {
    let error = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "remove-version",
        "--registry-id",
        "registry.testnet",
    ])
    .expect_err("remove-version needs --version-key or --all");
    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}

#[test]
fn clear_deployments_defaults_beneficiary_to_registry() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "clear-deployments",
        "--registry-id",
        "registry.testnet",
    ])
    .expect("clear-deployments should parse");
    let Command::Registry {
        command: RegistryNs::ClearDeployments(cmd),
    } = cli.command
    else {
        panic!("expected clear-deployments");
    };
    assert_eq!(cmd.beneficiary_id().as_str(), "registry.testnet");
    assert!(!cmd.force());
}
