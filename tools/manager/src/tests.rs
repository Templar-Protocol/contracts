use std::sync::Mutex;

use clap::{CommandFactory, Parser};

use super::cli::Cli;

static ENV_LOCK: Mutex<()> = Mutex::new(());

mod deploy_script;
mod ft;
mod market;
mod plan;
mod proxy_oracle;
mod redstone;
mod registry;
mod storage;

#[test]
fn help_lists_all_top_level_commands() {
    let rendered = Cli::command().render_long_help().to_string();
    for command in [
        "account",
        "contract",
        "registry",
        "storage",
        "ft",
        "market",
        "proxy-oracle",
        "proxy-oracle-owner",
        "proxy-oracle-governance",
        "redstone",
        "recover-nep141",
        "op",
        "read",
        "write",
    ] {
        assert!(rendered.contains(command), "help is missing `{command}`");
    }
}

#[test]
fn parses_op_get() {
    let cli = Cli::try_parse_from(["tmplrmgr", "op", "get", "--operation-id", "op-123"])
        .expect("op get should parse");
    assert!(matches!(
        cli.command,
        super::cli::Command::Op {
            command: super::commands::OpNs::Get(_)
        }
    ));
}

#[test]
fn parses_recover_nep141_args() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "recover-nep141",
        "--token-id",
        "usdt.testnet",
        "--beneficiary-id",
        "treasury.testnet",
        "--force",
    ])
    .expect("recover-nep141 should parse");
    match cli.command {
        super::cli::Command::RecoverNep141(args) => {
            assert_eq!(args.token_id.as_str(), "usdt.testnet");
            assert_eq!(args.beneficiary_id.as_str(), "treasury.testnet");
            assert!(args.force);
        }
        _ => panic!("expected recover-nep141"),
    }
}

#[test]
fn parses_read_fallback_with_json() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "read",
        "contract.getVersion",
        "--json",
        r#"{"contract_id":"market.testnet"}"#,
    ])
    .expect("read fallback should parse");

    match cli.command {
        super::cli::Command::Read(call) => {
            assert_eq!(call.method, "contract.getVersion");
            assert!(call.json.is_some());
        }
        _ => panic!("expected Read variant"),
    }
}

#[test]
fn parses_write_fallback_with_json() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "write",
        "registry.removeVersion",
        "--json",
        r#"{"registry_id":"registry.testnet","version_key":"v1"}"#,
    ])
    .expect("write fallback should parse");

    match cli.command {
        super::cli::Command::Write(call) => {
            assert_eq!(call.method, "registry.removeVersion");
            assert!(call.json.is_some());
        }
        _ => panic!("expected Write variant"),
    }
}

#[test]
fn idempotency_key_requires_gateway_store_url() {
    let error = Cli::try_parse_from([
        "tmplrmgr",
        "--idempotency-key",
        "retry-key",
        "write",
        "tx.transfer",
        "--json",
        r#"{"receiver_id":"receiver.testnet","amount":"1"}"#,
    ])
    .expect_err("idempotency key should require durable operation store configuration");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}

#[tokio::test]
async fn invalid_secret_key_error_does_not_echo_input() {
    let secret = "not-a-real-secret-key";
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "--signer-id",
        "signer.testnet",
        "--secret-key",
        secret,
        "read",
        "account.get",
        "--json",
        r#"{"account_id":"signer.testnet"}"#,
    ])
    .expect("secret key should parse as an opaque string at the clap boundary");

    let error = match super::build_context(&cli).await {
        Ok(_) => panic!("invalid secret key should be rejected after clap parsing"),
        Err(error) => error,
    };

    let message = error.to_string();
    assert_eq!(message, "invalid --secret-key");
    assert!(!message.contains(secret));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
// env mutex must span the await to keep SECRET_KEY set and serialize env across tests
async fn secret_key_env_satisfies_signer_configuration() {
    let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
    let original = std::env::var_os("SECRET_KEY");
    std::env::set_var(
        "SECRET_KEY",
        "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q",
    );

    let result = async {
        let cli = Cli::try_parse_from([
            "tmplrmgr",
            "--signer-id",
            "signer.testnet",
            "read",
            "account.get",
            "--json",
            r#"{"account_id":"signer.testnet"}"#,
        ])
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;

        super::build_context(&cli).await
    }
    .await;

    match original {
        Some(value) => std::env::set_var("SECRET_KEY", value),
        None => std::env::remove_var("SECRET_KEY"),
    }

    result.expect("env-provided SECRET_KEY should configure a signed client");
}

#[test]
fn read_fallback_rejects_missing_json() {
    let error = Cli::try_parse_from(["tmplrmgr", "read", "account.get"])
        .expect_err("read fallback should require --json or --json-file");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}
