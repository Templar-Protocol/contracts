use serde_json::json;

use super::command::command;
use super::GatewayCli;

mod ft;
mod registry;
mod storage;

#[test]
fn lists_tx_gateway_namespace() {
    let rendered = command().render_long_help().to_string();

    assert!(rendered.contains("tx"));
}

#[test]
fn parses_nested_account_delete_json_command() {
    let matches = command()
        .try_get_matches_from([
            "tmplrmgr",
            "account",
            "delete",
            "--json",
            r#"{"account_id":"old.near","beneficiary_id":"owner.near"}"#,
        ])
        .expect("account delete JSON command should parse");

    let (namespace, namespace_matches) = matches.subcommand().expect("namespace");
    let (method, _) = namespace_matches.subcommand().expect("method");

    assert_eq!(namespace, "account");
    assert_eq!(method, "delete");
}

#[test]
fn renders_tx_method_help() {
    let rendered = command()
        .find_subcommand_mut("tx")
        .expect("tx namespace should exist")
        .render_long_help()
        .to_string();

    assert!(rendered.contains("deployAndInit"));
    assert!(rendered.contains("functionCall"));
}

#[test]
fn lists_operation_gateway_namespace() {
    let rendered = command().render_long_help().to_string();

    assert!(rendered.contains("op"));
}

#[test]
fn parses_operation_get_json_command_with_store() {
    let cli = parse_cli([
        "tmplrmgr",
        "--gateway-store-url",
        "postgres://user:password@localhost/gateway",
        "op",
        "get",
        "--json",
        r#"{"operation_id":"00000000-0000-0000-0000-000000000000"}"#,
    ]);

    assert_eq!(cli.rpc_method(), "op.get");
    assert_eq!(
        cli.params(),
        &json!({ "operation_id": "00000000-0000-0000-0000-000000000000" })
    );
}

#[test]
fn operation_get_requires_gateway_store_url() {
    let matches = command()
        .try_get_matches_from([
            "tmplrmgr",
            "op",
            "get",
            "--json",
            r#"{"operation_id":"00000000-0000-0000-0000-000000000000"}"#,
        ])
        .expect("op.get JSON command should parse before store validation");

    let error = match GatewayCli::from_matches(&matches) {
        Ok(_) => panic!("op.get should require durable operation store configuration"),
        Err(error) => error,
    };

    assert!(error
        .to_string()
        .contains("op.get requires --gateway-store-url"));
}

#[test]
fn idempotency_key_requires_gateway_store_url() {
    let error = command()
        .try_get_matches_from([
            "tmplrmgr",
            "--idempotency-key",
            "retry-key",
            "tx",
            "transfer",
            "--json",
            r#"{"receiver_id":"receiver.testnet","amount":"1"}"#,
        ])
        .expect_err("idempotency key should require durable operation store configuration");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}

#[test]
fn invalid_secret_key_error_does_not_echo_input() {
    let secret = "not-a-real-secret-key";
    let matches = command()
        .try_get_matches_from([
            "tmplrmgr",
            "--signer-id",
            "signer.testnet",
            "--secret-key",
            secret,
            "account",
            "get",
            "--json",
            r#"{"account_id":"signer.testnet"}"#,
        ])
        .expect("secret key should parse as an opaque string at the clap boundary");

    let error = match GatewayCli::from_matches(&matches) {
        Ok(_) => panic!("invalid secret key should be rejected after clap parsing"),
        Err(error) => error,
    };

    let message = error.to_string();
    assert_eq!(message, "invalid --secret-key");
    assert!(!message.contains(secret));
}

#[test]
fn non_typed_method_still_rejects_missing_json() {
    let error = command()
        .try_get_matches_from(["tmplrmgr", "account", "get"])
        .expect_err("non-typed gateway method should require JSON parameters");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
}

pub(super) fn parse_cli(args: impl IntoIterator<Item = &'static str>) -> GatewayCli {
    let matches = command()
        .try_get_matches_from(args)
        .expect("typed gateway command should parse");

    GatewayCli::from_matches(&matches).expect("typed gateway params should load")
}
