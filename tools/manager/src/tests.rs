use std::sync::Mutex;

use clap::{CommandFactory, Parser};

use super::cli::Cli;

static ENV_LOCK: Mutex<()> = Mutex::new(());

mod deploy_script;
mod ft;
mod market;
mod oracle;
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
        "oracle",
        "proxy-oracle",
        "proxy-oracle-owner",
        "proxy-oracle-governance",
        "pyth",
        "redstone",
        "recover-nep141",
        "read",
        "write",
    ] {
        assert!(rendered.contains(command), "help is missing `{command}`");
    }
}

#[test]
fn parses_recover_nep141_args() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "recover-nep141",
            "--token-id",
            "usdt.testnet",
            "--beneficiary-id",
            "treasury.testnet",
            "--force",
        ]
        .into_iter()
        .chain(CREDS),
    )
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

const TEST_SECRET_KEY: &str = "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q";

/// Signer credentials appended to write-command argv in parse tests, so the
/// structural `SignerArgs` are satisfied. Shared by the submodules.
const CREDS: [&str; 4] = [
    "--signer-id",
    "signer.testnet",
    "--secret-key",
    TEST_SECRET_KEY,
];

#[test]
fn parses_write_fallback_with_json() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "write",
        "registry.removeVersion",
        "--json",
        r#"{"registry_id":"registry.testnet","version_key":"v1"}"#,
        "--signer-id",
        "signer.testnet",
        "--secret-key",
        TEST_SECRET_KEY,
    ])
    .expect("write fallback should parse");

    match cli.command {
        super::cli::Command::Write(call) => {
            assert_eq!(call.call.method, "registry.removeVersion");
            assert!(call.call.json.is_some());
            call.signer.resolve().expect("credentials should resolve");
        }
        _ => panic!("expected Write variant"),
    }
}

/// `write` flattens the oracle source flags because it may dispatch an `oracle.*`
/// update, but only the dispatched method builds a source. The Lazer token must
/// therefore stay optional, or every `write` — `market.borrow` included — would demand
/// it. Asserted against clap's metadata rather than by parsing: a parse test would pass
/// whenever the developer's environment happens to set `PYTH_LAZER_API_KEY`.
#[test]
fn write_fallback_does_not_require_a_lazer_key() {
    let write = Cli::command()
        .find_subcommand("write")
        .expect("write is a subcommand")
        .clone();
    let api_key = write
        .get_arguments()
        .find(|arg| arg.get_id() == "pyth_lazer_api_key")
        .expect("write flattens the oracle source flags");

    assert!(
        !api_key.is_required_set(),
        "--pyth-lazer-api-key must not be required by `write`"
    );
}

#[test]
fn write_command_requires_credentials() {
    // With credentials structural on the write, omitting them is a parse error —
    // no build or network work is reachable.
    let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
    let restore = clear_credential_env();

    let result = Cli::try_parse_from([
        "tmplrmgr",
        "write",
        "registry.removeVersion",
        "--json",
        r#"{"registry_id":"registry.testnet","version_key":"v1"}"#,
    ]);

    // Restore before asserting: a panic here must not leak cleared env vars into
    // later tests sharing this process.
    restore();
    let error = result.expect_err("a write with no credentials should fail to parse");
    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}

#[test]
fn read_command_rejects_credentials() {
    // Reads don't flatten the signer, so credentials are an unexpected argument.
    let error = Cli::try_parse_from([
        "tmplrmgr",
        "read",
        "account.get",
        "--json",
        r#"{"account_id":"signer.testnet"}"#,
        "--secret-key",
        TEST_SECRET_KEY,
    ])
    .expect_err("credentials on a read should fail to parse");

    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn invalid_secret_key_error_does_not_echo_input() {
    let secret = "not-a-real-secret-key";
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "write",
        "registry.removeVersion",
        "--json",
        r#"{"registry_id":"registry.testnet","version_key":"v1"}"#,
        "--signer-id",
        "signer.testnet",
        "--secret-key",
        secret,
    ])
    .expect("secret key should parse as an opaque string at the clap boundary");

    let error = match cli.command {
        super::cli::Command::Write(call) => call
            .signer
            .resolve()
            .expect_err("invalid secret key should be rejected"),
        _ => panic!("expected Write variant"),
    };

    let message = error.to_string();
    assert_eq!(message, "invalid --secret-key");
    assert!(!message.contains(secret));
}

#[test]
fn signer_env_satisfies_write_credentials() {
    // Scripted/CI usage relies on SIGNER_ID/SECRET_KEY env sourcing satisfying the
    // structural credentials with no explicit flags.
    let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
    let original_signer = std::env::var_os("SIGNER_ID");
    let original_secret = std::env::var_os("SECRET_KEY");
    std::env::set_var("SIGNER_ID", "signer.testnet");
    std::env::set_var("SECRET_KEY", TEST_SECRET_KEY);

    let result = (|| {
        let cli = Cli::try_parse_from([
            "tmplrmgr",
            "write",
            "registry.removeVersion",
            "--json",
            r#"{"registry_id":"registry.testnet","version_key":"v1"}"#,
        ])
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;

        match cli.command {
            super::cli::Command::Write(call) => call.signer.resolve().map(|_| ()),
            _ => anyhow::bail!("expected Write variant"),
        }
    })();

    restore_env("SIGNER_ID", original_signer);
    restore_env("SECRET_KEY", original_secret);

    result.expect("env-provided credentials should satisfy a write command");
}

/// Clear the credential env vars (under `ENV_LOCK`) so a "missing credentials"
/// parse test isn't satisfied by an ambient `SIGNER_ID`/`SECRET_KEY`. Returns a
/// closure that restores the originals.
fn clear_credential_env() -> impl FnOnce() {
    let original_signer = std::env::var_os("SIGNER_ID");
    let original_secret = std::env::var_os("SECRET_KEY");
    std::env::remove_var("SIGNER_ID");
    std::env::remove_var("SECRET_KEY");
    move || {
        restore_env("SIGNER_ID", original_signer);
        restore_env("SECRET_KEY", original_secret);
    }
}

fn restore_env(key: &str, original: Option<std::ffi::OsString>) {
    match original {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
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
