//! Local operator-readiness checks.

use std::{
    fs::{self, OpenOptions},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    artifacts::{
        inspect_existing_release_artifact, release_cache_root, ArtifactSpec,
        ExistingReleaseArtifact, RELEASE_REPO, RELEASE_TAG,
    },
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
    checks.push(artifact_cache_writable_check());
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
    writable_dir_check("manifest_writable", "manifest", parent)
}

pub(super) fn artifact_cache_writable_check() -> DoctorCheck {
    let root = match release_cache_root() {
        Ok(root) => root,
        Err(error) => return DoctorCheck::fail("artifact_cache_writable", format!("{error:#}")),
    };
    writable_dir_check("artifact_cache_writable", "artifact cache root", &root)
}

/// Shared probe: warn when the directory does not exist yet (deploy will
/// create it), fail when it is not a directory, else create/remove a probe
/// file.
pub(super) fn writable_dir_check(name: &str, what: &str, dir: &Path) -> DoctorCheck {
    if !dir.exists() {
        return DoctorCheck::warn(
            name,
            format!(
                "{what} {} does not exist yet; deploy will try to create it",
                dir.display()
            ),
        );
    }
    if !dir.is_dir() {
        return DoctorCheck::fail(name, format!("{what} {} is not a directory", dir.display()));
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let probe = dir.join(format!(
        ".tmplr-soroban-vault-cli-write-test-{}-{nanos}",
        std::process::id()
    ));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            DoctorCheck::pass(name, format!("{what} {} is writable", dir.display()))
        }
        Err(error) => DoctorCheck::fail(
            name,
            format!("cannot write {what} {}: {error}", dir.display()),
        ),
    }
}

pub(super) fn artifact_doctor_checks(cli: &Cli) -> Vec<DoctorCheck> {
    // Offline, zero-residue readiness probe: read-only inspection of the
    // release cache and workspace seed. No directories are created, nothing
    // is downloaded, and no state is mutated.
    ArtifactSpec::stack_artifacts(true, true)
        .into_iter()
        .map(
            |spec| match inspect_existing_release_artifact(&cli.workspace_path, spec) {
                Ok(existing) => artifact_check(&spec, &existing),
                Err(error) => {
                    DoctorCheck::fail(format!("artifact_{}", spec.key), format!("{error:#}"))
                }
            },
        )
        .collect()
}

pub(super) fn artifact_check(
    spec: &ArtifactSpec,
    existing: &ExistingReleaseArtifact,
) -> DoctorCheck {
    let name = format!("artifact_{}", spec.key);
    match existing {
        ExistingReleaseArtifact::Cache { path, sha256 }
        | ExistingReleaseArtifact::WorkspaceSeed { path, sha256 } => DoctorCheck::pass(
            name,
            format!(
                "verified release pin at {} (sha256 {sha256})",
                path.display()
            ),
        ),
        ExistingReleaseArtifact::IgnoredWorkspace {
            cache_path,
            workspace_path,
            reason,
        } => DoctorCheck::warn(
            name,
            format!(
                "workspace output {} is not the pinned release ({reason}) and will be ignored; deploy downloads {RELEASE_TAG} from {RELEASE_REPO} into {}",
                workspace_path.display(),
                cache_path.display()
            ),
        ),
        ExistingReleaseArtifact::Missing { cache_path } => DoctorCheck::warn(
            name,
            format!(
                "{} is not cached and no verified workspace seed exists; deploy downloads the \
                 pinned release {RELEASE_TAG} from {RELEASE_REPO} GitHub releases, or deploy \
                 --build builds package {}",
                cache_path.display(),
                spec.package
            ),
        ),
    }
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
