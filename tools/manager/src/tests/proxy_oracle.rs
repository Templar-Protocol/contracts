use clap::Parser;
use serde_json::{json, Value};

use crate::cli::{Cli, Command};
use crate::commands::proxy_oracle::ProxyOracleNs;
use crate::commands::proxy_oracle_governance::ProxyOracleGovernanceNs;

const PRICE_ID: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

/// Parse a `create-proposal` invocation and return the built gateway spec.
fn create_proposal(
    args: &[&str],
) -> anyhow::Result<templar_gateway_methods_spec::proxy_oracle_governance::CreateProposal> {
    let mut full = vec!["tmplrmgr", "proxy-oracle-governance", "create-proposal"];
    full.extend_from_slice(args);
    match Cli::try_parse_from(full)
        .expect("create-proposal should parse")
        .command
    {
        Command::ProxyOracleGovernance {
            command: ProxyOracleGovernanceNs::CreateProposal(cmd),
        } => {
            let id = cmd.id().expect("these tests pass --id explicitly");
            cmd.into_spec(id)
        }
        _ => panic!("expected create-proposal"),
    }
}

/// The `operation` field of a parsed proposal, as JSON.
fn operation_json(args: &[&str]) -> Value {
    let spec = create_proposal(args).expect("into spec");
    serde_json::to_value(&spec).unwrap()["operation"].clone()
}

#[test]
fn set_role_operation_grants_and_revokes() {
    let granted = operation_json(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "1",
        "set-role",
        "--account-id",
        "op.testnet",
        "--role",
        "admin",
    ]);
    assert_eq!(
        granted,
        json!({ "SetRole": { "account_id": "op.testnet", "role": "Admin", "set": true } })
    );

    let revoked = operation_json(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "1",
        "set-role",
        "--account-id",
        "op.testnet",
        "--role",
        "circuit-breaker-operator",
        "--revoke",
    ]);
    assert_eq!(
        revoked,
        json!({
            "SetRole": { "account_id": "op.testnet", "role": "CircuitBreakerOperator", "set": false }
        })
    );
}

#[test]
fn admin_function_call_operation_defaults_and_encodes_args() {
    let op = operation_json(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "admin-function-call",
        "--method",
        "own_accept_owner",
        "--deposit",
        "1 yoctoNEAR",
    ]);
    assert_eq!(
        op,
        json!({
            "AdminFunctionCall": {
                "method_name": "own_accept_owner",
                "args": "e30=", // base64("{}")
                "attached_deposit": "1",
                "gas": "30000000000000", // 30 Tgas default
            }
        })
    );
}

#[test]
fn set_action_ttl_operation_builds_kind_and_ttl() {
    let op = operation_json(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "5",
        "set-action-ttl",
        "--kind",
        "admin-upgrade",
        "--new-ttl",
        "86400000000000",
    ]);
    assert_eq!(
        op,
        json!({ "SetActionTtl": { "kind": "AdminUpgrade", "new_ttl": "86400000000000" } })
    );
}

#[test]
fn configure_circuit_breakers_operation_builds_config() {
    let op = operation_json(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "2",
        "configure-circuit-breakers",
        "--price-id",
        PRICE_ID,
        "--sample-interval-ns",
        "1000",
        "--history-len",
        "8",
    ]);
    assert_eq!(
        op["ConfigureCircuitBreakers"]["config"]["history_len"],
        json!(8)
    );
    assert_eq!(
        op["ConfigureCircuitBreakers"]["config"]["sample_interval_ns"],
        json!("1000")
    );
}

#[test]
fn admin_upgrade_operation_reads_code_file() {
    let wasm = b"\0asm\x01\0\0\0upgrade";
    let path = std::env::temp_dir().join(format!(
        "tmplrmgr-upgrade-{}-{}.wasm",
        std::process::id(),
        line!()
    ));
    std::fs::write(&path, wasm).expect("write wasm fixture");

    let op = operation_json(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "9",
        "admin-upgrade",
        "--code-file",
        path.to_str().unwrap(),
    ]);
    std::fs::remove_file(&path).ok();

    // Base64VecU8 serializes bytes as a base64 string; migrate_args defaults empty.
    assert!(op["AdminUpgrade"]["code"].is_string());
    assert_eq!(op["AdminUpgrade"]["migrate_args"], json!(""));
}

#[test]
fn requested_ttl_defaults_to_zero_and_is_carried() {
    let spec = create_proposal(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "3",
        "--requested-ttl",
        "42",
        "remove-circuit-breaker",
        "--price-id",
        PRICE_ID,
        "--breaker-id",
        "0",
    ])
    .expect("into spec");
    let json = serde_json::to_value(&spec).unwrap();
    assert_eq!(json["requested_ttl"], json!("42"));
    assert_eq!(
        json["operation"]["RemoveCircuitBreaker"]["breaker_id"],
        json!(0)
    );
}

#[test]
fn create_proposal_id_is_optional_for_auto_fetch() {
    // With --id omitted, the id is left unresolved (the dispatcher fetches the
    // governance contract's next proposal id before writing).
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "proxy-oracle-governance",
        "create-proposal",
        "--governance-id",
        "gov.testnet",
        "set-proxy",
        "--price-id",
        PRICE_ID,
    ])
    .expect("create-proposal without --id should parse");
    match cli.command {
        Command::ProxyOracleGovernance {
            command: ProxyOracleGovernanceNs::CreateProposal(cmd),
        } => {
            assert_eq!(cmd.id(), None);
            assert!(!cmd.execute_when_ready());
            assert_eq!(cmd.governance_id().as_str(), "gov.testnet");
            // A resolved id flows through to the spec.
            let spec = cmd.into_spec(7).expect("into spec");
            assert_eq!(serde_json::to_value(&spec).unwrap()["id"], json!(7));
        }
        _ => panic!("expected create-proposal"),
    }
}

#[test]
fn create_proposal_execute_when_ready_flag() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "proxy-oracle-governance",
        "create-proposal",
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "--execute-when-ready",
        "admin-function-call",
        "--method",
        "own_accept_owner",
        "--deposit",
        "1 yoctoNEAR",
    ])
    .expect("create-proposal --execute-when-ready should parse");
    match cli.command {
        Command::ProxyOracleGovernance {
            command: ProxyOracleGovernanceNs::CreateProposal(cmd),
        } => assert!(cmd.execute_when_ready()),
        _ => panic!("expected create-proposal"),
    }
}

#[test]
fn execute_proposal_when_ready_flag() {
    for (args, expected) in [
        (vec!["--governance-id", "gov.testnet", "--id", "2"], false),
        (
            vec![
                "--governance-id",
                "gov.testnet",
                "--id",
                "2",
                "--when-ready",
            ],
            true,
        ),
    ] {
        let mut full = vec!["tmplrmgr", "proxy-oracle-governance", "execute-proposal"];
        full.extend_from_slice(&args);
        let cli = Cli::try_parse_from(full).expect("execute-proposal should parse");
        match cli.command {
            Command::ProxyOracleGovernance {
                command: ProxyOracleGovernanceNs::ExecuteProposal(cmd),
            } => {
                assert_eq!(cmd.when_ready(), expected);
                assert_eq!(cmd.id(), 2);
            }
            _ => panic!("expected execute-proposal"),
        }
    }
}

#[test]
fn governance_create_builds_init_args() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "proxy-oracle-governance",
        "create",
        "--registry-id",
        "registry.testnet",
        "--name",
        "proxy-governance-market",
        "--version-key",
        "gov@1",
        "--proxy-oracle-id",
        "proxy-oracle-market.registry.testnet",
        "--admin-id",
        "operator.testnet",
        "--ttl-default",
        "0",
        "--deposit",
        "3.5 NEAR",
    ])
    .expect("governance create should parse");

    let deploy = match cli.command {
        Command::ProxyOracleGovernance {
            command: ProxyOracleGovernanceNs::Create(cmd),
        } => cmd.parse().expect("into deploy spec"),
        _ => panic!("expected governance create"),
    };

    // Wraps registry.deploy with typed init args.
    assert_eq!(deploy.name, "proxy-governance-market");
    let init: Value = serde_json::from_slice(&deploy.init_args.0).expect("init args are json");
    assert_eq!(
        init["proxy_oracle_id"],
        "proxy-oracle-market.registry.testnet"
    );
    assert_eq!(init["admin_id"], "operator.testnet");
    // All 11 operation-kind TTL fields present and uniform.
    let ttls = init["ttls"].as_object().expect("ttls object");
    assert_eq!(ttls.len(), 11);
    let zero = &ttls["set_proxy"];
    for (_, v) in ttls {
        assert_eq!(v, zero);
    }
}

#[test]
fn governance_role_reads_parse() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "proxy-oracle-governance",
        "list-role",
        "--governance-id",
        "gov.testnet",
        "--role",
        "proxy-configuration-manager",
        "--offset",
        "0",
        "--count",
        "10",
    ])
    .expect("list-role should parse");
    let spec = match cli.command {
        Command::ProxyOracleGovernance {
            command: ProxyOracleGovernanceNs::ListRole(cmd),
        } => cmd.parse(),
        _ => panic!("expected list-role"),
    };
    assert_eq!(
        serde_json::to_value(&spec).unwrap(),
        json!({
            "governance_id": "gov.testnet",
            "role": "ProxyConfigurationManager",
            "offset": 0,
            "count": 10
        })
    );
}

#[test]
fn oracle_update_prices_collects_repeated_ids() {
    let borrow = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "proxy-oracle",
        "update-prices",
        "--oracle-id",
        "proxy.testnet",
        "--price-id",
        PRICE_ID,
        "--price-id",
        borrow,
    ])
    .expect("update-prices should parse");
    let spec = match cli.command {
        Command::ProxyOracle {
            command: ProxyOracleNs::UpdatePrices(cmd),
        } => cmd.parse().expect("into spec"),
        _ => panic!("expected update-prices"),
    };
    assert_eq!(spec.price_ids.len(), 2);
}

#[test]
fn add_circuit_breaker_breaker_id_is_optional_and_resolvable() {
    // Omitting --breaker-id marks the proposal for auto-resolution.
    let Command::ProxyOracleGovernance {
        command: ProxyOracleGovernanceNs::CreateProposal(mut cmd),
    } = Cli::try_parse_from([
        "tmplrmgr",
        "proxy-oracle-governance",
        "create-proposal",
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "add-circuit-breaker",
        "--price-id",
        PRICE_ID,
        "--breaker-file",
        "/does/not/need/to/exist.json",
    ])
    .expect("add-circuit-breaker should parse")
    .command
    else {
        panic!("expected create-proposal");
    };
    assert_eq!(cmd.unresolved_breaker_price_id(), Some(PRICE_ID));
    cmd.set_breaker_id(4);
    // Once resolved, it is no longer flagged for auto-fetch.
    assert_eq!(cmd.unresolved_breaker_price_id(), None);

    // An explicit --breaker-id needs no resolution.
    let Command::ProxyOracleGovernance {
        command: ProxyOracleGovernanceNs::CreateProposal(explicit),
    } = Cli::try_parse_from([
        "tmplrmgr",
        "proxy-oracle-governance",
        "create-proposal",
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "add-circuit-breaker",
        "--price-id",
        PRICE_ID,
        "--breaker-id",
        "7",
        "--breaker-file",
        "/does/not/need/to/exist.json",
    ])
    .expect("add-circuit-breaker with --breaker-id should parse")
    .command
    else {
        panic!("expected create-proposal");
    };
    assert_eq!(explicit.unresolved_breaker_price_id(), None);
}
