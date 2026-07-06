use clap::Parser;
use serde_json::{json, Value};

use crate::cli::{Cli, Command};
use crate::commands::proxy_oracle::{ProxyOracleGovernanceNs, ProxyOracleOwnerNs};
use crate::commands::registry::RegistryNs;

const COLLATERAL_PRICE_ID: &str =
    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[test]
fn registry_deploy_defaults_init_args_to_null() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "deploy",
        "--registry-id",
        "registry.testnet",
        "--name",
        "proxy-oracle-market",
        "--version-key",
        "proxy@1",
        "--deposit",
        "3.5 NEAR",
    ])
    .expect("registry deploy should parse");

    let params = match cli.command {
        Command::Registry {
            command: RegistryNs::Deploy(cmd),
        } => cmd.parse().expect("deploy should parse"),
        _ => panic!("expected Registry::Deploy"),
    };

    let params_json = serde_json::to_value(&params).unwrap();
    assert_eq!(params_json["init_args"], json!("bnVsbA=="));
    serde_json::from_value::<templar_gateway_methods_spec::registry::Deploy>(params_json)
        .expect("typed registry.deploy params should match the gateway spec");
}

#[test]
fn registry_deploy_reads_init_args_file() {
    let init_args = br#"{"owner_id":"operator.testnet"}"#;
    let path = std::env::temp_dir().join(format!(
        "tmplrmgr-deploy-init-args-{}-{}.json",
        std::process::id(),
        line!(),
    ));
    std::fs::write(&path, init_args).expect("write init args fixture");

    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "deploy",
        "--registry-id",
        "registry.testnet",
        "--name",
        "proxy-oracle",
        "--version-key",
        "proxy-oracle@1",
        "--init-args-file",
        path.to_str().expect("fixture path is unicode"),
        "--deposit",
        "1 yoctoNEAR",
    ])
    .expect("registry deploy with init-args-file should parse");

    let params = match cli.command {
        Command::Registry {
            command: RegistryNs::Deploy(cmd),
        } => cmd.parse().expect("deploy should parse"),
        _ => panic!("expected Registry::Deploy"),
    };

    std::fs::remove_file(&path).expect("remove init args fixture");

    let params_json = serde_json::to_value(&params).unwrap();
    assert_eq!(params_json["init_args"], json!(base64_of(init_args)));
}

#[test]
fn proxy_oracle_owner_typed_commands_parse() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "proxy-oracle-owner",
        "propose-owner",
        "--oracle-id",
        "proxy.registry.testnet",
        "--account-id",
        "operator.testnet",
    ])
    .expect("propose-owner should parse");

    let params = match cli.command {
        Command::ProxyOracleOwner {
            command: ProxyOracleOwnerNs::ProposeOwner(cmd),
        } => cmd.parse(),
        _ => panic!("expected ProxyOracleOwner::ProposeOwner"),
    };

    serde_json::from_value::<templar_gateway_methods_spec::proxy_oracle_owner::ProposeOwner>(
        serde_json::to_value(&params).unwrap(),
    )
    .expect("typed proposeOwner params should match the gateway spec");

    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "proxy-oracle-owner",
        "accept-owner",
        "--oracle-id",
        "proxy.registry.testnet",
    ])
    .expect("accept-owner should parse");

    let params = match cli.command {
        Command::ProxyOracleOwner {
            command: ProxyOracleOwnerNs::AcceptOwner(cmd),
        } => cmd.parse(),
        _ => panic!("expected ProxyOracleOwner::AcceptOwner"),
    };

    serde_json::from_value::<templar_gateway_methods_spec::proxy_oracle_owner::AcceptOwner>(
        serde_json::to_value(&params).unwrap(),
    )
    .expect("typed acceptOwner params should match the gateway spec");
}

#[test]
fn governance_create_proposal_reshapes_legacy_proxy_file() {
    let legacy_entries = json!([{
        "source": {"Request": {"Pyth": {
            "oracle_id": "pyth-oracle.testnet",
            "price_id": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
        }}},
        "weight": 1
    }]);
    let proxy_file = write_legacy_proxy_fixture(&legacy_entries);

    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "proxy-oracle-governance",
        "create-proposal",
        "--governance-id",
        "proxy.registry.testnet",
        "--id",
        "0",
        "--operation",
        "set-proxy",
        "--price-id",
        COLLATERAL_PRICE_ID,
        "--proxy-file",
        proxy_file.to_str().expect("fixture path is unicode"),
    ])
    .expect("create-proposal with proxy-file should parse");

    let params = match cli.command {
        Command::ProxyOracleGovernance {
            command: ProxyOracleGovernanceNs::CreateProposal(cmd),
        } => cmd.parse().expect("create-proposal should parse"),
        _ => panic!("expected ProxyOracleGovernance::CreateProposal"),
    };

    std::fs::remove_file(&proxy_file).expect("remove proxy fixture");

    let params_json = serde_json::to_value(&params).unwrap();
    let proxy = &params_json["operation"]["SetProxy"]["proxy"];
    assert_eq!(proxy["aggregator"]["MedianLow"]["sources"], legacy_entries);
    assert_eq!(proxy["aggregator"]["MedianLow"]["min_sources"], json!(1));

    serde_json::from_value::<templar_gateway_methods_spec::proxy_oracle_governance::CreateProposal>(
        params_json,
    )
    .expect("typed createProposal params should match the gateway spec");
}

#[test]
fn governance_execute_proposal_typed_args_parse() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "proxy-oracle-governance",
        "execute-proposal",
        "--governance-id",
        "proxy.registry.testnet",
        "--id",
        "0",
    ])
    .expect("execute-proposal should parse");

    let params = match cli.command {
        Command::ProxyOracleGovernance {
            command: ProxyOracleGovernanceNs::ExecuteProposal(cmd),
        } => cmd.parse(),
        _ => panic!("expected ProxyOracleGovernance::ExecuteProposal"),
    };

    serde_json::from_value::<templar_gateway_methods_spec::proxy_oracle_governance::ExecuteProposal>(
        serde_json::to_value(&params).unwrap(),
    )
    .expect("typed executeProposal params should match the gateway spec");
}

#[test]
fn create_proposal_set_proxy_requires_price_id_and_proxy_file() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "proxy-oracle-governance",
        "create-proposal",
        "--governance-id",
        "proxy.registry.testnet",
        "--id",
        "0",
        "--operation",
        "set-proxy",
    ])
    .expect("set-proxy operation should parse before parse validation");

    let error = match cli.command {
        Command::ProxyOracleGovernance {
            command: ProxyOracleGovernanceNs::CreateProposal(cmd),
        } => cmd
            .parse()
            .expect_err("set-proxy operation should require price-id and proxy-file"),
        _ => panic!("expected ProxyOracleGovernance::CreateProposal"),
    };

    assert!(error.to_string().contains("--price-id") || error.to_string().contains("--proxy-file"));
}

#[test]
fn json_fallback_still_works_for_create_proposal() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "write",
        "proxyOracleGovernance.createProposal",
        "--json",
        r#"{"governance_id":"proxy.registry.testnet","id":0,"operation":{"SetProxy":{"id":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","proxy":{"aggregator":{"MedianLow":{"sources":[],"min_sources":1}},"freshness_filter":{"max_age_ns":"1","max_clock_drift_ns":"1"}}}},"requested_ttl":"0"}"#,
    ])
    .expect("write fallback should parse");

    let params = match cli.command {
        Command::Write(call) => {
            let json_str = call.json.expect("json should be present");
            serde_json::from_str(&json_str).expect("json should parse")
        }
        _ => panic!("expected Write variant"),
    };

    serde_json::from_value::<templar_gateway_methods_spec::proxy_oracle_governance::CreateProposal>(
        params,
    )
    .expect("JSON fallback createProposal params should match the gateway spec");
}

fn write_legacy_proxy_fixture(entries: &Value) -> std::path::PathBuf {
    let payload = json!({
        "aggregator": {
            "method": "MedianLow",
            "filter": {
                "min_sources": 1,
                "max_age": "60000000000",
                "max_clock_drift": "10000000000"
            }
        },
        "entries": entries,
    });
    let path = std::env::temp_dir().join(format!(
        "tmplrmgr-proxy-fixture-{}-{}.json",
        std::process::id(),
        line!(),
    ));
    std::fs::write(&path, payload.to_string()).expect("write proxy fixture");
    path
}

fn base64_of(bytes: &[u8]) -> String {
    use templar_gateway_types::Base64Bytes;
    serde_json::to_value(Base64Bytes(bytes.to_vec()))
        .expect("Base64Bytes serializes to a string")
        .as_str()
        .expect("base64 value is a string")
        .to_owned()
}
