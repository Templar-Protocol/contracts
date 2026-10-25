use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use clap::Parser;

use crate::{workspace_loader, ArtifactId, ArtifactMetadata, ReleaseArtifact};

mod scheduler;
use scheduler::prebuild_all;

const DEFAULT_MAX_JOBS: usize = 4;
const DEFAULT_BUILD_TIMEOUT_SECS: u64 = 30 * 60;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value_os_t = default_workspace_root())]
    workspace_root: PathBuf,

    #[arg(long, env = "PREBUILD_TEST_CONTRACTS_JOBS", default_value_t = default_jobs())]
    jobs: usize,

    #[arg(
        long,
        env = "PREBUILD_TEST_CONTRACTS_TIMEOUT_SECS",
        default_value_t = DEFAULT_BUILD_TIMEOUT_SECS
    )]
    timeout_secs: u64,

    /// Artifact to build; repeat or separate values with commas.
    #[arg(
        long = "artifact",
        value_name = "ARTIFACT",
        value_enum,
        ignore_case = true,
        value_delimiter = ','
    )]
    artifacts: Vec<ArtifactId>,

    /// Report whether the selected artifacts are already built, without building them.
    #[arg(long)]
    check: bool,

    /// Resolve a release tag to its catalogued artifact and exit. Writes
    /// `key=value` lines (`artifact`, `package`, `version`, `source_path`,
    /// `target`, `asset`) for a CI step to append to
    /// `$GITHUB_OUTPUT`.
    ///
    /// A tag naming a contract of the Soroban proxy-oracle stack is resolved
    /// like a NEAR one, and additionally carries `kind=soroban`: the lines for
    /// a NEAR tag are exactly what they have always been, so a caller written
    /// against the NEAR recipe reads an absent `kind` as "the NEAR recipe".
    ///
    /// Exit codes carry the outcome: `0` resolved, `2` not a catalogued
    /// artifact, `1` failure — so a failed lookup cannot masquerade as an
    /// intentional skip and green a release with no WASM.
    #[arg(long, value_name = "TAG", conflicts_with_all = ["check", "artifacts"])]
    resolve: Option<String>,
}

/// `--resolve` found no catalogued artifact for the tag. Distinct from failure.
const EXIT_NOT_CATALOGUED: u8 = 2;

/// Printed on the skip path so a caller can tell it from clap's usage error,
/// which also exits 2. The workflow builds a tag it did not come from, so a tag
/// predating `--resolve` would otherwise skip silently and publish no WASM.
const SKIP_SENTINEL: &str = "skip=not-catalogued";

pub fn main() -> ExitCode {
    let args = Args::parse();

    if let Some(tag) = &args.resolve {
        let Some(artifact) = ReleaseArtifact::from_release_tag(tag) else {
            println!("{SKIP_SENTINEL}");
            eprintln!("{tag} names no catalogued WASM artifact");
            return ExitCode::from(EXIT_NOT_CATALOGUED);
        };

        // Observed, not guessed: release-plz set this crate's Cargo.toml
        // version immediately before creating the tag. Both catalogs describe
        // packages of this workspace, so one metadata read answers either.
        let version = match workspace_loader::get_metadata(&args.workspace_root).map(|workspace| {
            workspace_loader::find_package(&workspace, artifact.package_name())
                .map(|package| package.version.clone())
        }) {
            Ok(Some(version)) => version,
            Ok(None) => {
                eprintln!("{} is not in the workspace", artifact.package_name());
                return ExitCode::FAILURE;
            }
            Err(error) => {
                eprintln!("failed to read cargo metadata: {error}");
                return ExitCode::FAILURE;
            }
        };

        // The catalog names files `<artifact>@<version>.tsv` and orders
        // releases by `major.minor.patch`, so a prerelease or build-metadata
        // version has no entry it could be recorded under. Refusing it here is
        // what keeps that state unreachable: the build and the asset upload
        // both come after this step, and a published Release cannot be
        // withdrawn once its bytes are cited.
        if !version.pre.is_empty() || !version.build.is_empty() {
            eprintln!(
                "{} {version} is not `major.minor.patch`; the artifact catalog \
                 cannot record it, so this release would produce bytes no \
                 consumer could verify",
                artifact.package_name(),
            );
            return ExitCode::FAILURE;
        }

        for line in resolve_lines(artifact, &version.to_string()) {
            println!("{line}");
        }
        return ExitCode::SUCCESS;
    }

    let artifacts = selected_artifacts(&args.artifacts);
    let result = if args.check {
        check_all(&args.workspace_root, &artifacts)
    } else {
        prebuild_all(
            &args.workspace_root,
            args.jobs.max(1),
            Duration::from_secs(args.timeout_secs),
            artifacts,
        )
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

/// The `$GITHUB_OUTPUT` lines one resolved tag produces. Kept apart from `main`
/// so the naming contract is a value a test can assert rather than a process's
/// stdout: a NEAR tag's six lines, unchanged; a Soroban tag's, plus `kind`.
fn resolve_lines(artifact: ReleaseArtifact, version: &str) -> Vec<String> {
    let mut lines = vec![
        // The catalog key, and the name of the file `record-release` writes.
        format!("artifact={artifact}"),
        format!("package={}", artifact.package_name()),
        format!("version={version}"),
        format!("source_path={}", artifact.source_path()),
        format!("target={}", artifact.cargo_target_name()),
        format!("asset={}", artifact.asset_name(version)),
    ];
    // Printed only where the NEAR recipe cannot build the tag, so that adding
    // the Soroban stack cannot disturb what the existing NEAR path emits.
    if matches!(artifact, ReleaseArtifact::Soroban(_)) {
        lines.push("kind=soroban".to_owned());
    }
    lines
}

/// Report every selected artifact missing from `target/near`.
fn check_all(workspace_root: &Path, artifacts: &[&'static ArtifactMetadata]) -> Result<(), ()> {
    let metadata = workspace_loader::get_metadata(workspace_root)
        .map_err(|error| eprintln!("failed to read cargo metadata: {error}"))?;
    let target_dir = metadata.target_directory.as_std_path();
    let mut missing = false;

    for artifact in artifacts {
        let path = workspace_loader::target_near_wasm_path_from_meta(
            target_dir,
            artifact.cargo_target_name,
        );
        if !path.is_file() {
            eprintln!("missing prebuilt artifact: {}", path.display());
            missing = true;
        }
    }

    if missing {
        Err(())
    } else {
        Ok(())
    }
}

fn default_jobs() -> usize {
    let available = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    available.clamp(1, DEFAULT_MAX_JOBS)
}

fn default_workspace_root() -> PathBuf {
    workspace_loader::discover_workspace_root().unwrap_or_else(|| PathBuf::from("."))
}

fn selected_artifacts(selection: &[ArtifactId]) -> Vec<&'static ArtifactMetadata> {
    if selection.is_empty() {
        return ArtifactId::ALL.iter().map(|id| id.metadata()).collect();
    }

    let selected = selection.iter().copied().collect::<HashSet<_>>();
    ArtifactId::ALL
        .iter()
        .filter(|id| selected.contains(id))
        .map(|id| id.metadata())
        .collect()
}

#[cfg(test)]
mod tests;
