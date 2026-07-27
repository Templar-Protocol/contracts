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

    /// Resolve a release tag to its catalogued artifact and exit, without
    /// building. Writes `key=value` lines (`package`, `version`, `source_path`,
    /// `target`, `asset`) for a CI step to append to `$GITHUB_OUTPUT`.
    ///
    /// The three outcomes a release job must tell apart are exit codes, not
    /// output: `0` resolved, `2` not a catalogued NEAR artifact (a Soroban tag,
    /// say), `1` genuine failure. Signalling "not catalogued" by printing
    /// nothing would let a failed lookup masquerade as an intentional skip and
    /// green a release with no WASM.
    #[arg(long, value_name = "TAG", conflicts_with_all = ["check", "artifacts"])]
    resolve: Option<String>,
}

/// `--resolve` found no catalogued artifact for the tag. Distinct from failure.
const EXIT_NOT_CATALOGUED: u8 = 2;

pub fn main() -> ExitCode {
    let args = Args::parse();
    let artifacts = selected_artifacts(&args.artifacts);

    if let Some(tag) = &args.resolve {
        // Purely a catalog lookup: no `cargo metadata`, because the version is
        // in the tag. The tag is parsed by `artifact_from_release_tag`, the
        // tested inverse of the function that builds it, rather than by pulling
        // the string apart in shell.
        let Some((artifact, version)) = crate::artifact_from_release_tag(tag) else {
            eprintln!("{tag} names no catalogued NEAR artifact");
            return ExitCode::from(EXIT_NOT_CATALOGUED);
        };
        let metadata = artifact.metadata();
        println!("package={}", metadata.package_name);
        println!("version={version}");
        println!("source_path={}", metadata.source_path);
        println!("target={}", metadata.cargo_target_name);
        println!("asset={}", crate::asset_name(artifact, version));
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
