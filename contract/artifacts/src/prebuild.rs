use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use clap::Parser;

use crate::{workspace_loader, ArtifactId, ArtifactMetadata};

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
    /// Exit codes carry the outcome: `0` resolved, `2` not a catalogued NEAR
    /// artifact, `1` failure — so a failed lookup cannot masquerade as an
    /// intentional skip and green a release with no WASM.
    #[arg(long, value_name = "TAG", conflicts_with_all = ["check", "artifacts"])]
    resolve: Option<String>,
}

/// `--resolve` found no catalogued artifact for the tag. Distinct from failure.
const EXIT_NOT_CATALOGUED: u8 = 2;

pub fn main() -> ExitCode {
    let args = Args::parse();

    if let Some(tag) = &args.resolve {
        let Some(artifact) = crate::artifact_from_release_tag(tag) else {
            eprintln!("{tag} names no catalogued NEAR artifact");
            return ExitCode::from(EXIT_NOT_CATALOGUED);
        };
        let metadata = artifact.metadata();

        // Observed, not guessed: release-plz set this crate's Cargo.toml
        // version immediately before creating the tag.
        let version = match workspace_loader::get_metadata(&args.workspace_root).map(|workspace| {
            workspace_loader::find_package(&workspace, metadata.package_name)
                .map(|package| package.version.clone())
        }) {
            Ok(Some(version)) => version,
            Ok(None) => {
                eprintln!("{} is not in the workspace", metadata.package_name);
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
                metadata.package_name,
            );
            return ExitCode::FAILURE;
        }

        // The catalog key, and the name of the file `record-release` writes.
        println!("artifact={artifact}");
        println!("package={}", metadata.package_name);
        println!("version={version}");
        println!("source_path={}", metadata.source_path);
        println!("target={}", metadata.cargo_target_name);
        // The one place a new release's asset gets its name; existing releases
        // have theirs recorded in `releases/`.
        println!("asset={}-{version}.wasm", metadata.cargo_target_name);
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
