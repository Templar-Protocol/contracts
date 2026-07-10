use clap::Parser;

use super::CREDS;
use crate::cli::{Cli, Command};
use crate::commands::RedstoneNs;

#[test]
fn write_prices_decodes_base64_payload() {
    // "hello" encoded as standard base64.
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "redstone",
            "write-prices",
            "--oracle-id",
            "redstone.testnet",
            "--feed-id",
            "BTC",
            "--feed-id",
            "ETH",
            "--payload-base64",
            "aGVsbG8=",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("write-prices should parse");
    let params = match cli.command {
        Command::Redstone {
            command: RedstoneNs::WritePrices(a),
        } => a.try_into_spec().expect("write-prices should decode"),
        _ => panic!("expected redstone write-prices"),
    };

    assert_eq!(params.payload.0, b"hello");
    assert_eq!(params.feed_ids.len(), 2);
    assert_eq!(&*params.feed_ids[0], "BTC");

    let json = serde_json::to_value(&params).unwrap();
    serde_json::from_value::<templar_gateway_methods_spec::redstone::WritePrices>(json)
        .expect("write-prices params should match the gateway spec");
}

#[test]
fn write_prices_rejects_invalid_base64() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "redstone",
            "write-prices",
            "--oracle-id",
            "redstone.testnet",
            "--feed-id",
            "BTC",
            "--payload-base64",
            "not valid base64!!!",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("write-prices should parse at the clap boundary");
    let error = match cli.command {
        Command::Redstone {
            command: RedstoneNs::WritePrices(a),
        } => a
            .try_into_spec()
            .expect_err("invalid base64 should be rejected"),
        _ => panic!("expected redstone write-prices"),
    };
    assert!(error.to_string().contains("base64"));
}

#[test]
fn list_role_maps_role_arg_to_snake_case() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "redstone",
        "list-role",
        "--oracle-id",
        "redstone.testnet",
        "--role",
        "modify-roles",
    ])
    .expect("list-role should parse");
    let params = match cli.command {
        Command::Redstone {
            command: RedstoneNs::ListRole(a),
        } => a.into_spec(),
        _ => panic!("expected redstone list-role"),
    };

    let json = serde_json::to_value(&params).unwrap();
    assert_eq!(json["role"], "modify_roles");
    serde_json::from_value::<templar_gateway_methods_spec::redstone::ListRole>(json)
        .expect("list-role params should match the gateway spec");
}

#[test]
fn create_prod_preset_builds_config_init_args() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "redstone",
            "create",
            "--registry-id",
            "registry.testnet",
            "--name",
            "redstone",
            "--version-key",
            "redstone@1",
            "--prod",
            "--deposit",
            "3.5 NEAR",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("redstone create --prod should parse");
    let deploy = match cli.command {
        Command::Redstone {
            command: RedstoneNs::Create(a),
        } => a.try_into_spec().expect("into deploy spec"),
        _ => panic!("expected redstone create"),
    };

    // Wraps registry.deploy; init args carry the built-in prod config.
    assert_eq!(deploy.name, "redstone");
    let init: serde_json::Value =
        serde_json::from_slice(&deploy.init_args.0).expect("init args are json");
    assert_eq!(
        init["config"],
        serde_json::to_value(templar_common::oracle::redstone::config::prod()).unwrap()
    );
}

#[test]
fn create_requires_exactly_one_config_source() {
    // --prod and --test are mutually exclusive.
    let error = Cli::try_parse_from(
        [
            "tmplrmgr",
            "redstone",
            "create",
            "--registry-id",
            "registry.testnet",
            "--name",
            "redstone",
            "--version-key",
            "redstone@1",
            "--prod",
            "--test",
            "--deposit",
            "3.5 NEAR",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect_err("--prod with --test should be rejected");
    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
}

/// `redstone update-prices` is gone: `oracle update-red-stone` fetches the payload
/// inside the gateway rather than spawning the bridge CLI-side.
#[test]
fn update_prices_is_no_longer_a_redstone_subcommand() {
    let error = Cli::try_parse_from(
        [
            "tmplrmgr",
            "redstone",
            "update-prices",
            "--oracle-id",
            "redstone.testnet",
            "--feed-id",
            "BTC",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect_err("redstone update-prices should no longer parse");

    assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
}
