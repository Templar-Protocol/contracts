use std::str::FromStr;

use clap::Parser;
use serde_json::{json, Value};

use near_sdk::json_types::{Base58CryptoHash, Base64VecU8, U128};
use near_sdk::Gas;

use super::{parse_create_proposal, parse_governance, CREDS};
use crate::cli::{Cli, Command};
use crate::commands::proxy_oracle::{ProxyOracleGovernanceNs, ProxyOracleNs};
use templar_common::upgrade::UpgradeSource;
use templar_common::Nanoseconds;
use templar_primitives::Decimal;
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{CircuitBreaker, StepwiseChange};
use templar_proxy_oracle_near_governance_common::{Operation, OperationKind, Role};

const PRICE_ID: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
struct RemoveFileOnDrop<'a>(&'a std::path::Path);

impl Drop for RemoveFileOnDrop<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// Parse a `create-proposal` invocation and return the built gateway spec.
fn create_proposal(
    args: &[&str],
) -> anyhow::Result<templar_gateway_methods_spec::proxy_oracle_governance::CreateProposal> {
    let cmd = parse_create_proposal(args.iter().copied());
    let id = cmd.id().expect("these tests pass --id explicitly");
    cmd.try_into_spec(id)
}

#[test]
fn governance_and_gov_parse_into_the_nested_command() {
    for alias in ["governance", "gov"] {
        let cli = Cli::try_parse_from([
            "tmplrmgr",
            "proxy-oracle",
            alias,
            "get-proxy-oracle-id",
            "--governance-id",
            "gov.testnet",
        ])
        .expect("governance alias should parse");

        match cli.command {
            Command::ProxyOracle {
                command: ProxyOracleNs::Governance(ProxyOracleGovernanceNs::GetProxyOracleId(_)),
            } => {}
            _ => panic!("expected nested governance command"),
        }
    }

    let error = Cli::try_parse_from(["tmplrmgr", "proxy-oracle", "g"])
        .expect_err("removed single-letter alias should be rejected");
    assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
}

/// The typed `operation` a `create-proposal` invocation parses into.
fn operation(args: &[&str]) -> Operation {
    create_proposal(args).expect("into spec").operation
}

/// Parse a `proxy-oracle create` invocation and build its gateway spec.
fn oracle_create(
    version_key: &str,
    owner_id: Option<&str>,
) -> anyhow::Result<templar_gateway_methods_spec::proxy_oracle::Create> {
    let mut args = vec![
        "tmplrmgr",
        "proxy-oracle",
        "create",
        "--registry-id",
        "registry.testnet",
        "--name",
        "proxy-oracle-btc",
        "--version-key",
        version_key,
        "--deposit",
        "5 NEAR",
    ];
    if let Some(owner_id) = owner_id {
        args.extend_from_slice(&["--owner-id", owner_id]);
    }

    match Cli::try_parse_from(args.into_iter().chain(CREDS))
        .expect("proxy-oracle create should parse")
        .command
    {
        Command::ProxyOracle {
            command: ProxyOracleNs::Create(cmd),
        } => cmd.try_into_spec(),
        _ => panic!("expected proxy-oracle create"),
    }
}

#[test]
fn upgrade_reads_local_wasm_and_carries_the_migration() {
    let wasm = b"\0asm\x01\0\0\0proxy-oracle";
    let path = std::env::temp_dir().join(format!(
        "tmplrmgr-proxy-upgrade-{}-{}.wasm",
        std::process::id(),
        line!()
    ));
    std::fs::write(&path, wasm).expect("write WASM fixture");

    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "proxy-oracle",
            "upgrade",
            "--oracle-id",
            "oracle.testnet",
            "--wasm",
            path.to_str().unwrap(),
            "--migration",
            "v0",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("upgrade command should parse");

    let spec = match cli.command {
        Command::ProxyOracle {
            command: ProxyOracleNs::Upgrade(cmd),
        } => cmd.try_into_spec().expect("read WASM"),
        _ => panic!("expected proxy-oracle upgrade"),
    };
    std::fs::remove_file(&path).ok();

    assert_eq!(spec.wasm.0, wasm);
    assert_eq!(
        serde_json::to_value(spec).unwrap()["migration"],
        serde_json::json!("v0")
    );
}

const V0_3_0: &str = "templar-proxy-oracle-near-contract@0.3.0#ab";
const V0_2_0: &str = "templar-proxy-oracle-near-contract@0.2.0#ab";
const UNREADABLE: &str = "templar-proxy-oracle-near-contract-0.3.0";

/// `--owner-id` reaches the gateway spec as a typed account id, not a JSON string
/// interpolated by the caller.
#[test]
fn create_carries_owner_id_into_the_gateway_spec() {
    let spec = oracle_create(V0_3_0, Some("gov.testnet")).expect("into spec");

    assert_eq!(spec.name, "proxy-oracle-btc");
    assert_eq!(spec.owner_id, Some("gov.testnet".parse().unwrap()));

    let json = serde_json::to_value(&spec).unwrap();
    serde_json::from_value::<templar_gateway_methods_spec::proxy_oracle::Create>(json)
        .expect("create params should match the gateway spec");
}

/// The guard fires only when an owner is named: an old `new` silently drops one,
/// and an unreadable key cannot be shown to accept one. With no owner there is
/// nothing to drop, so neither may be refused.
#[rstest::rstest]
#[case::honored(V0_3_0, Some("gov.testnet"), None)]
#[case::ignored(V0_2_0, Some("gov.testnet"), Some("takes no arguments"))]
#[case::unreadable(UNREADABLE, Some("gov.testnet"), Some("cannot tell whether"))]
#[case::no_owner_new(V0_3_0, None, None)]
#[case::no_owner_old(V0_2_0, None, None)]
#[case::no_owner_unreadable(UNREADABLE, None, None)]
fn create_guards_owner_id_against_the_named_version(
    #[case] version_key: &str,
    #[case] owner_id: Option<&str>,
    #[case] expected_error: Option<&str>,
) {
    let result = oracle_create(version_key, owner_id);

    match expected_error {
        None => {
            let spec = result.expect("into spec");
            assert_eq!(spec.owner_id, owner_id.map(|id| id.parse().unwrap()));
        }
        Some(expected) => {
            let error = result.expect_err("guard should refuse");
            let message = format!("{error:#}");
            assert!(message.contains(expected), "{message}");
        }
    }
}

#[test]
fn set_role_operation_grants_and_revokes() {
    let granted = operation(&[
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
        Operation::SetRole {
            account_id: "op.testnet".parse().unwrap(),
            role: Role::Admin,
            set: true,
        }
    );

    let revoked = operation(&[
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
        Operation::SetRole {
            account_id: "op.testnet".parse().unwrap(),
            role: Role::CircuitBreakerOperator,
            set: false,
        }
    );
}

#[test]
fn admin_function_call_operation_defaults_and_encodes_args() {
    let op = operation(&[
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
        Operation::AdminFunctionCall {
            method_name: "own_accept_owner".to_string(),
            args: Base64VecU8(b"{}".to_vec()),
            attached_deposit: U128(1),
            gas: Gas::from_tgas(30),
        }
    );
}

#[test]
fn set_action_ttl_operation_builds_kind_and_ttl() {
    let op = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "5",
        "set-action-ttl",
        "--kind",
        "admin-upgrade",
        "--new-ttl",
        "1d",
    ]);
    assert_eq!(
        op,
        Operation::SetActionTtl {
            kind: OperationKind::AdminUpgrade,
            new_ttl: Nanoseconds::from_secs(24 * 60 * 60),
        }
    );
}

#[test]
fn configure_circuit_breakers_operation_builds_config() {
    let op = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "2",
        "configure-circuit-breakers",
        "--price-id",
        PRICE_ID,
        "--sample-interval",
        "1000ns",
        "--history-len",
        "8",
    ]);
    match op {
        Operation::ConfigureCircuitBreakers { config, .. } => {
            assert_eq!(config.history_len, 8);
            assert_eq!(config.sample_interval_ns, Nanoseconds::from_ns(1000));
        }
        other => panic!("expected ConfigureCircuitBreakers, got {other:?}"),
    }
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

    let op = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "9",
        "admin-upgrade",
        "--code-file",
        path.to_str().unwrap(),
    ]);
    std::fs::remove_file(&path).ok();

    assert_eq!(
        op,
        Operation::AdminUpgrade {
            code: UpgradeSource::Code(Base64VecU8(wasm.to_vec())),
            migrate_args: Base64VecU8(Vec::new()),
        }
    );
}

#[test]
fn admin_upgrade_accepts_a_global_hash() {
    // 32 base58 '1's decode to 32 zero bytes — a valid code hash.
    let by_hash = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "9",
        "admin-upgrade",
        "--global-hash",
        "11111111111111111111111111111111",
    ]);
    assert_eq!(
        by_hash,
        Operation::AdminUpgrade {
            code: UpgradeSource::GlobalHash(Base58CryptoHash::from([0u8; 32])),
            migrate_args: Base64VecU8(Vec::new()),
        }
    );
}

#[test]
fn self_upgrade_operation_targets_the_governance_contract() {
    let op = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "9",
        "self-upgrade",
        "--global-hash",
        "11111111111111111111111111111111",
    ]);
    assert_eq!(
        op,
        Operation::SelfUpgrade {
            code: UpgradeSource::GlobalHash(Base58CryptoHash::from([0u8; 32])),
            migrate_args: Base64VecU8(Vec::new()),
        }
    );
}

#[test]
fn requested_ttl_defaults_to_zero_and_is_carried() {
    // Explicit --requested-ttl is carried through (in nanoseconds).
    let spec = create_proposal(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "3",
        "--requested-ttl",
        "42ns",
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

    // Omitting --requested-ttl defaults it to zero.
    let defaulted = create_proposal(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "3",
        "remove-circuit-breaker",
        "--price-id",
        PRICE_ID,
        "--breaker-id",
        "0",
    ])
    .expect("into spec");
    assert_eq!(
        serde_json::to_value(&defaulted).unwrap()["requested_ttl"],
        json!("0")
    );
}

#[test]
fn create_proposal_id_is_optional_for_auto_fetch() {
    // With --id omitted, the id is left unresolved (the dispatcher fetches the
    // governance contract's next proposal id before writing).
    let cmd = parse_create_proposal([
        "--governance-id",
        "gov.testnet",
        "set-proxy",
        "--price-id",
        PRICE_ID,
    ]);
    assert_eq!(cmd.id(), None);
    assert!(!cmd.execute_when_ready());
    assert_eq!(cmd.governance_id().as_str(), "gov.testnet");
    // A resolved id flows through to the spec.
    let spec = cmd.try_into_spec(7).expect("into spec");
    assert_eq!(serde_json::to_value(&spec).unwrap()["id"], json!(7));
}

#[test]
fn create_proposal_execute_when_ready_flag() {
    let cmd = parse_create_proposal([
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
    ]);
    assert!(cmd.execute_when_ready());
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
        let command = parse_governance(["execute-proposal"].into_iter().chain(args).chain(CREDS));
        match command {
            ProxyOracleGovernanceNs::ExecuteProposal(cmd) => {
                assert_eq!(cmd.when_ready(), expected);
                assert_eq!(cmd.id(), 2);
            }
            _ => panic!("expected execute-proposal"),
        }
    }
}

#[test]
fn governance_create_builds_init_args() {
    let deploy = match parse_governance(
        [
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
            "0ns",
            "--deposit",
            "3.5 NEAR",
        ]
        .into_iter()
        .chain(CREDS),
    ) {
        ProxyOracleGovernanceNs::Create(cmd) => cmd.try_into_spec().expect("into deploy spec"),
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
    // All 12 operation-kind TTL fields present and uniform.
    let ttls = init["ttls"].as_object().expect("ttls object");
    assert_eq!(ttls.len(), 12);
    let zero = &ttls["set_proxy"];
    for (_, v) in ttls {
        assert_eq!(v, zero);
    }
}

#[test]
fn governance_role_reads_parse() {
    let spec = match parse_governance([
        "list-role",
        "--governance-id",
        "gov.testnet",
        "--role",
        "proxy-configuration-manager",
        "--offset",
        "0",
        "--count",
        "10",
    ]) {
        ProxyOracleGovernanceNs::ListRole(cmd) => cmd.into_spec(),
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
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "proxy-oracle",
            "update-prices",
            "--oracle-id",
            "proxy.testnet",
            "--price-id",
            PRICE_ID,
            "--price-id",
            borrow,
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("update-prices should parse");
    let spec = match cli.command {
        Command::ProxyOracle {
            command: ProxyOracleNs::UpdatePrices(cmd),
        } => cmd.into_spec(),
        _ => panic!("expected update-prices"),
    };
    assert_eq!(spec.price_ids.len(), 2);
}

#[test]
fn add_circuit_breaker_breaker_id_is_optional_and_resolvable() {
    let breaker_file = std::env::temp_dir().join(format!(
        "tmplrmgr-circuit-breaker-{}-{}.json",
        std::process::id(),
        line!()
    ));
    let _breaker_file_cleanup = RemoveFileOnDrop(&breaker_file);
    let breaker = CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: Decimal::from_str("0.1").unwrap(),
    });
    std::fs::write(
        &breaker_file,
        serde_json::to_vec(&breaker).expect("serialize breaker fixture"),
    )
    .expect("write breaker fixture");
    let breaker_file_arg = breaker_file.to_str().expect("fixture path is unicode");

    // Omitting --breaker-id marks the proposal for auto-resolution.
    let parse_unresolved = || {
        parse_create_proposal([
            "--governance-id",
            "gov.testnet",
            "--id",
            "0",
            "add-circuit-breaker",
            "--price-id",
            PRICE_ID,
            "--breaker-file",
            breaker_file_arg,
        ])
    };
    let unresolved = parse_unresolved();
    let expected = crate::commands::proxy_oracle::parse_price_identifier(PRICE_ID).unwrap();
    assert_eq!(unresolved.unresolved_breaker_price_id(), Some(expected));
    let error = unresolved
        .try_into_spec(0)
        .expect_err("building a spec must reject an unresolved breaker id");
    assert!(
        error.to_string().contains("breaker id must be resolved"),
        "{error:#}"
    );

    let mut resolved = parse_unresolved();
    resolved.set_breaker_id(4);
    // Once resolved, it is no longer flagged for auto-fetch.
    assert_eq!(resolved.unresolved_breaker_price_id(), None);
    let spec = resolved
        .try_into_spec(0)
        .expect("resolved breaker id should build");
    let operation =
        serde_json::to_value(spec).expect("serialize proposal spec")["operation"].clone();
    assert_eq!(operation["AddCircuitBreaker"]["breaker_id"], json!(4));

    // An explicit --breaker-id needs no resolution.
    let explicit = parse_create_proposal([
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
    ]);
    assert_eq!(explicit.unresolved_breaker_price_id(), None);
}
