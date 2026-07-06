use clap::Parser;

use crate::cli::{Cli, Command};
use crate::commands::RedstoneNs;

#[test]
fn write_prices_decodes_base64_payload() {
    // "hello" encoded as standard base64.
    let cli = Cli::try_parse_from([
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
    ])
    .expect("write-prices should parse");
    let params = match cli.command {
        Command::Redstone {
            command: RedstoneNs::WritePrices(a),
        } => a.parse().expect("write-prices should decode"),
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
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "redstone",
        "write-prices",
        "--oracle-id",
        "redstone.testnet",
        "--feed-id",
        "BTC",
        "--payload-base64",
        "not valid base64!!!",
    ])
    .expect("write-prices should parse at the clap boundary");
    let error = match cli.command {
        Command::Redstone {
            command: RedstoneNs::WritePrices(a),
        } => a.parse().expect_err("invalid base64 should be rejected"),
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
        } => a.parse(),
        _ => panic!("expected redstone list-role"),
    };

    let json = serde_json::to_value(&params).unwrap();
    assert_eq!(json["role"], "modify_roles");
    serde_json::from_value::<templar_gateway_methods_spec::redstone::ListRole>(json)
        .expect("list-role params should match the gateway spec");
}
