use std::sync::Mutex;

use clap::{CommandFactory, Parser};

use super::cli::{Cli, Command};
use super::commands::proxy_oracle::{CreateProposal, ProxyOracleGovernanceNs, ProxyOracleNs};
use super::commands::signer::PrintFormat;

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
        "owner",
        "pyth",
        "redstone",
        "recover-nep141",
        "read",
        "write",
    ] {
        assert!(rendered.contains(command), "help is missing `{command}`");
    }
    assert!(
        Cli::command()
            .find_subcommand("proxy-oracle-governance")
            .is_none(),
        "legacy governance command is still top-level"
    );
    assert!(
        !rendered.contains("proxy-oracle-owner"),
        "help still lists the removed `proxy-oracle-owner` command"
    );
}

#[test]
fn owner_uses_concise_subcommands() {
    let command = Cli::command();
    let owner = command
        .find_subcommand("owner")
        .expect("owner command should exist");
    let names = owner
        .get_subcommands()
        .map(clap::Command::get_name)
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        ["get", "get-proposed", "propose", "accept", "renounce"]
    );
    assert!(Cli::try_parse_from(["tmplrmgr", "proxy-oracle-owner"]).is_err());
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

fn try_parse_write<'a>(args: impl IntoIterator<Item = &'a str>) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(
        [
            "tmplrmgr",
            "write",
            "registry.removeVersion",
            "--json",
            r#"{"registry_id":"registry.testnet","version_key":"v1"}"#,
        ]
        .into_iter()
        .chain(args),
    )
}

fn try_parse_governance<'a>(
    args: impl IntoIterator<Item = &'a str>,
) -> Result<ProxyOracleGovernanceNs, clap::Error> {
    let command = Cli::try_parse_from(
        ["tmplrmgr", "proxy-oracle", "governance"]
            .into_iter()
            .chain(args),
    )?
    .command;
    let Command::ProxyOracle {
        command: ProxyOracleNs::Governance(command),
    } = command
    else {
        unreachable!("governance argv prefix always selects the nested namespace");
    };
    Ok(command)
}

fn parse_governance<'a>(args: impl IntoIterator<Item = &'a str>) -> ProxyOracleGovernanceNs {
    try_parse_governance(args).expect("governance command should parse")
}

fn parse_create_proposal<'a>(args: impl IntoIterator<Item = &'a str>) -> CreateProposal {
    // Credentials belong to `create-proposal` and must precede its operation subcommand.
    let ProxyOracleGovernanceNs::CreateProposal(command) =
        parse_governance(["create-proposal"].into_iter().chain(CREDS).chain(args))
    else {
        panic!("expected create-proposal");
    };
    command
}

#[test]
fn parses_write_fallback_with_json() {
    let cli = try_parse_write(CREDS).expect("write fallback should parse");

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
fn write_requires_secret_key_or_print() {
    // Omitting both execution credentials and plan mode is a parse error, so no
    // build or network work is reachable.
    let result = with_cleared_credential_env(|| try_parse_write(["--signer-id", "dao.near"]));
    let error = result.expect_err("a write needs --secret-key or --print");
    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}

#[test]
fn write_command_accepts_print_without_secret() {
    let result = with_cleared_credential_env(|| {
        try_parse_write(["--signer-id", "dao.near", "--print", "sputnik"])
    });
    let cli = result.expect("plan-only write should parse without a secret");
    let Command::Write(call) = cli.command else {
        panic!("expected Write variant");
    };
    assert_eq!(call.signer.print(), Some(PrintFormat::Sputnik));
}

#[test]
fn print_conflicts_with_secret_key() {
    let error = try_parse_write([
        "--signer-id",
        "dao.near",
        "--print",
        "json",
        "--secret-key",
        TEST_SECRET_KEY,
    ])
    .expect_err("plan and execution credentials must be mutually exclusive");

    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn print_conflicts_with_secret_key_from_environment() {
    let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
    let original_secret = std::env::var_os("SECRET_KEY");
    std::env::set_var("SECRET_KEY", TEST_SECRET_KEY);
    let result = try_parse_write(["--signer-id", "dao.near", "--print", "json"]);
    restore_env("SECRET_KEY", original_secret);

    let error = result.expect_err("environment secret must conflict with --print");
    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn public_key_requires_print_mode() {
    let error = with_cleared_credential_env(|| {
        try_parse_write([
            "--signer-id",
            "signer.testnet",
            "--public-key",
            "ed25519:5TMKtTtD5uuMF28ovo7vVge7oAu58eXjySJWTrwcEB5w",
        ])
    })
    .expect_err("--public-key is plan-only");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
    assert!(error.to_string().contains("--print <FORMAT>"));
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
    let error = try_parse_write(["--signer-id", "signer.testnet", "--secret-key", secret])
        .expect_err("invalid secret key should fail at the clap boundary");

    let message = error.to_string();
    assert!(message.contains("invalid --secret-key"), "{message}");
    assert!(!message.contains(secret), "secret leaked: {message}");
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
        let cli = try_parse_write([]).map_err(|error| anyhow::anyhow!(error.to_string()))?;

        match cli.command {
            super::cli::Command::Write(call) => call.signer.resolve().map(|_| ()),
            _ => anyhow::bail!("expected Write variant"),
        }
    })();

    restore_env("SIGNER_ID", original_signer);
    restore_env("SECRET_KEY", original_secret);

    result.expect("env-provided credentials should satisfy a write command");
}

/// Run `f` with ambient signer credentials cleared and environment mutation
/// serialized, then restore the original values.
fn with_cleared_credential_env<T>(f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
    let original_signer = std::env::var_os("SIGNER_ID");
    let original_secret = std::env::var_os("SECRET_KEY");
    std::env::remove_var("SIGNER_ID");
    std::env::remove_var("SECRET_KEY");
    let result = f();
    restore_env("SIGNER_ID", original_signer);
    restore_env("SECRET_KEY", original_secret);
    result
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
