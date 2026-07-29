//! Local operator-readiness checks.

use std::{
    fs::{self, OpenOptions},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    artifacts::ArtifactSpec,
    cli::Cli,
    stellar::{keys_address_source_account_args, CommandExecutor},
};

use super::{
    context::CommandContext,
    output::{DoctorCheck, DoctorResponse, DoctorStatus, Response},
};

pub(super) fn run_doctor<E: CommandExecutor>(context: &CommandContext<'_, E>) -> Response {
    let cli = context.cli();
    let executor = context.executor();
    let mut checks = Vec::new();

    let version_args = vec!["--version".to_string()];
    match executor.run("stellar", &version_args, &[], &[]) {
        Ok(output) => checks.push(DoctorCheck::pass(
            "stellar_version",
            first_nonempty_line(&output.stdout, &output.stderr)
                .unwrap_or("stellar CLI responded")
                .to_string(),
        )),
        Err(error) => checks.push(DoctorCheck::fail(
            "stellar_version",
            format!("stellar CLI is not runnable: {error}"),
        )),
    }

    checks.push(DoctorCheck::pass(
        "network",
        format!(
            "network={} passphrase={}",
            cli.network, cli.network_passphrase
        ),
    ));
    if let Some(rpc_url) = &cli.rpc_url {
        checks.push(DoctorCheck::pass("rpc_url", rpc_url.clone()));
    } else {
        checks.push(DoctorCheck::warn(
            "rpc_url",
            "no RPC URL override configured; Stellar CLI network config must provide one"
                .to_string(),
        ));
    }
    if cli.network == "mainnet" && !cli.allow_mainnet_write {
        checks.push(DoctorCheck::warn(
            "mainnet_guard",
            "mainnet is selected; write commands remain blocked until --allow-mainnet-write is passed"
                .to_string(),
        ));
    }

    checks.push(source_account_doctor_check(context));
    checks.push(manifest_writable_check(&cli.state));
    checks.extend(artifact_doctor_checks(cli));
    checks.extend(docker_mount_checks(cli));

    Response::Doctor(DoctorResponse {
        ok: checks
            .iter()
            .all(|check| check.status != DoctorStatus::Fail),
        checks,
    })
}

pub(super) fn source_account_doctor_check<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
) -> DoctorCheck {
    let cli = context.cli();
    let executor = context.executor();
    if cli.source_account.is_some() {
        return match context.stellar().source_public_address() {
            Ok(address) => DoctorCheck::pass(
                "source_account",
                format!("source identity/address resolves to {address}"),
            ),
            Err(error) => DoctorCheck::fail(
                "source_account",
                format!("source identity/address did not resolve: {error}"),
            ),
        };
    }
    if std::env::var_os("STELLAR_ACCOUNT").is_some() {
        return DoctorCheck::pass(
            "source_account",
            "STELLAR_ACCOUNT is set for child stellar signing; value is not inspected".to_string(),
        );
    }

    let (args, redacted_args) = keys_address_source_account_args(None, cli.config_dir.as_deref());
    match executor.run("stellar", &args, &redacted_args, &[]) {
        Ok(output) if !output.stdout.trim().is_empty() => DoctorCheck::pass(
            "source_account",
            format!(
                "default Stellar identity resolves to {}",
                output.stdout.trim()
            ),
        ),
        Ok(_) => DoctorCheck::warn(
            "source_account",
            "no --source-account, SOROBAN_IDENTITY, STELLAR_ACCOUNT, or default Stellar identity detected"
                .to_string(),
        ),
        Err(error) => DoctorCheck::warn(
            "source_account",
            format!("could not inspect default Stellar identity: {error}"),
        ),
    }
}

pub(super) fn manifest_writable_check(path: &Path) -> DoctorCheck {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return DoctorCheck::warn(
            "manifest_writable",
            format!("manifest path {} has no parent directory", path.display()),
        );
    };
    if !parent.exists() {
        return DoctorCheck::warn(
            "manifest_writable",
            format!(
                "manifest directory {} does not exist yet; deploy will try to create it",
                parent.display()
            ),
        );
    }
    if !parent.is_dir() {
        return DoctorCheck::fail(
            "manifest_writable",
            format!("manifest parent {} is not a directory", parent.display()),
        );
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let probe = parent.join(format!(
        ".tmplr-soroban-vault-cli-write-test-{}-{nanos}",
        std::process::id()
    ));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            DoctorCheck::pass(
                "manifest_writable",
                format!("manifest directory {} is writable", parent.display()),
            )
        }
        Err(error) => DoctorCheck::fail(
            "manifest_writable",
            format!(
                "cannot write manifest directory {}: {error}",
                parent.display()
            ),
        ),
    }
}

pub(super) fn artifact_doctor_checks(cli: &Cli) -> Vec<DoctorCheck> {
    let workspace_manifest = cli.workspace_path.join("Cargo.toml");
    ArtifactSpec::stack_artifacts(true, true)
        .into_iter()
        .map(|spec| {
            let wasm_path = spec.wasm_path(&cli.workspace_path);
            if wasm_path.exists() {
                DoctorCheck::pass(
                    format!("artifact_{}", spec.key),
                    format!("found {}", wasm_path.display()),
                )
            } else if workspace_manifest.exists() {
                DoctorCheck::warn(
                    format!("artifact_{}", spec.key),
                    format!(
                        "{} is missing; deploy --build can build package {}",
                        wasm_path.display(),
                        spec.package
                    ),
                )
            } else {
                DoctorCheck::fail(
                    format!("artifact_{}", spec.key),
                    format!(
                        "{} is missing and {} was not found",
                        wasm_path.display(),
                        workspace_manifest.display()
                    ),
                )
            }
        })
        .collect()
}

pub(super) fn docker_mount_checks(cli: &Cli) -> Vec<DoctorCheck> {
    if !Path::new("/.dockerenv").exists() {
        return vec![DoctorCheck::warn(
            "docker_mounts",
            "not running inside Docker; mount checks skipped".to_string(),
        )];
    }

    let mut checks = Vec::new();
    if cli.workspace_path.exists() {
        checks.push(DoctorCheck::pass(
            "docker_workspace_mount",
            format!("workspace path {} exists", cli.workspace_path.display()),
        ));
    } else {
        checks.push(DoctorCheck::fail(
            "docker_workspace_mount",
            format!("workspace path {} is missing", cli.workspace_path.display()),
        ));
    }

    let target = cli.workspace_path.join("target");
    if target.exists() {
        checks.push(DoctorCheck::pass(
            "docker_target_mount",
            format!("target path {} exists", target.display()),
        ));
    } else {
        checks.push(DoctorCheck::warn(
            "docker_target_mount",
            format!(
                "target path {} is missing; builds will not reuse host artifacts",
                target.display()
            ),
        ));
    }

    if let Some(config_dir) = &cli.config_dir {
        if config_dir.exists() {
            checks.push(DoctorCheck::pass(
                "docker_stellar_config_mount",
                format!("Stellar config path {} exists", config_dir.display()),
            ));
        } else {
            checks.push(DoctorCheck::warn(
                "docker_stellar_config_mount",
                format!(
                    "Stellar config path {} is missing; identities may not persist",
                    config_dir.display()
                ),
            ));
        }
    }
    checks
}

pub(super) fn first_nonempty_line<'a>(first: &'a str, second: &'a str) -> Option<&'a str> {
    first
        .lines()
        .chain(second.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
}
