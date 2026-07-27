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

    /// Print `<package_name> <source_path> <cargo_target_name> <version>` for
    /// each selected artifact and exit, without building. Lets shell tooling
    /// (`release-artifacts.yml`) read the catalog instead of re-deriving paths
    /// and drifting from it. The package name leads so a caller can select a row
    /// without passing `--artifact`, which cannot distinguish "not catalogued"
    /// from "the command failed".
    #[arg(long, conflicts_with = "check")]
    print_metadata: bool,
}

pub fn main() -> ExitCode {
    let args = Args::parse();
    let artifacts = selected_artifacts(&args.artifacts);

    if args.print_metadata {
        let metadata = match workspace_loader::get_metadata(&args.workspace_root) {
            Ok(metadata) => metadata,
            Err(error) => {
                eprintln!("failed to read cargo metadata: {error}");
                return ExitCode::FAILURE;
            }
        };
        for artifact in &artifacts {
            // Version comes from the crate's own Cargo.toml, so callers never
            // re-derive it (and cannot disagree with the drift check).
            let Some(package) = workspace_loader::find_package(&metadata, artifact.package_name)
            else {
                eprintln!(
                    "package {} not in workspace metadata",
                    artifact.package_name
                );
                return ExitCode::FAILURE;
            };
            println!(
                "{} {} {} {}",
                artifact.package_name,
                artifact.source_path,
                artifact.cargo_target_name,
                package.version,
            );
        }
        return ExitCode::SUCCESS;
    }

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
