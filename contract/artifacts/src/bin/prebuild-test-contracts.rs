use std::{
    collections::{HashSet, VecDeque},
    env,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::{Command, ExitCode, ExitStatus},
};

use clap::Parser;
use templar_contract_artifacts::{
    artifact_catalog, manifest_path, parse_artifact_id, ArtifactMetadata, ContractArtifact,
};

const JOBS_ENV: &str = "PREBUILD_TEST_CONTRACTS_JOBS";
const DEFAULT_MAX_JOBS: usize = 4;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value = ".")]
    workspace_root: PathBuf,

    #[arg(long, value_parser = parse_jobs)]
    jobs: Option<NonZeroUsize>,

    #[arg(
        long,
        help = "Use non-reproducible debug builds instead of reproducible builds"
    )]
    debug: bool,

    #[arg(
        long = "artifact",
        value_name = "ARTIFACT",
        value_parser = parse_artifact_id,
        value_delimiter = ',',
        help = "Artifact to build; repeat or separate values with commas"
    )]
    artifacts: Vec<ContractArtifact>,
}

struct RunningBuild {
    artifact: &'static ArtifactMetadata,
    child: std::process::Child,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildMode {
    Reproducible,
    Debug,
}

impl BuildMode {
    fn from_debug(debug: bool) -> Self {
        if debug {
            Self::Debug
        } else {
            Self::Reproducible
        }
    }

    const fn cargo_near_command(self) -> &'static str {
        match self {
            Self::Reproducible => "reproducible-wasm",
            Self::Debug => "non-reproducible-wasm",
        }
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    let jobs = args
        .jobs
        .or_else(jobs_from_env)
        .unwrap_or_else(default_jobs);
    let build_mode = BuildMode::from_debug(args.debug);
    let artifacts = selected_artifacts(&args.artifacts);

    match prebuild_all(&args.workspace_root, jobs, build_mode, artifacts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn parse_jobs(value: &str) -> Result<NonZeroUsize, String> {
    value
        .parse::<NonZeroUsize>()
        .map_err(|_| "jobs must be a positive integer".to_string())
}

fn jobs_from_env() -> Option<NonZeroUsize> {
    env::var(JOBS_ENV)
        .ok()
        .and_then(|value| parse_jobs(&value).ok())
}

fn default_jobs() -> NonZeroUsize {
    let available = std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1);
    NonZeroUsize::new(available.clamp(1, DEFAULT_MAX_JOBS)).unwrap_or(NonZeroUsize::MIN)
}

fn selected_artifacts(selection: &[ContractArtifact]) -> Vec<&'static ArtifactMetadata> {
    if selection.is_empty() {
        return artifact_catalog().iter().collect();
    }

    let selected = selection.iter().copied().collect::<HashSet<_>>();
    artifact_catalog()
        .iter()
        .filter(|artifact| selected.contains(&artifact.id))
        .collect()
}

fn prebuild_all(
    workspace_root: &Path,
    jobs: NonZeroUsize,
    build_mode: BuildMode,
    artifacts: Vec<&'static ArtifactMetadata>,
) -> Result<(), ()> {
    let mut pending = artifacts.into_iter().collect::<VecDeque<_>>();
    let mut running = Vec::<RunningBuild>::new();
    let mut failed = false;

    while !pending.is_empty() || !running.is_empty() {
        while running.len() < jobs.get() {
            let Some(artifact) = pending.pop_front() else {
                break;
            };

            match spawn_build(workspace_root, artifact, build_mode) {
                Ok(child) => running.push(RunningBuild { artifact, child }),
                Err(error) => {
                    eprintln!("failed to start {}: {error}", artifact.package_name);
                    failed = true;
                }
            }
        }

        if let Some(mut build) = running.pop() {
            match build.child.wait() {
                Ok(status) if status.success() => {
                    eprintln!("finished {}", build.artifact.package_name);
                }
                Ok(status) => {
                    report_failed_status(build.artifact, status);
                    failed = true;
                }
                Err(error) => {
                    eprintln!(
                        "failed while waiting for {}: {error}",
                        build.artifact.package_name
                    );
                    failed = true;
                }
            }
        }
    }

    if failed {
        Err(())
    } else {
        Ok(())
    }
}

fn spawn_build(
    workspace_root: &Path,
    artifact: &'static ArtifactMetadata,
    build_mode: BuildMode,
) -> std::io::Result<std::process::Child> {
    let manifest = workspace_root
        .join(manifest_path(artifact))
        .join("Cargo.toml");

    eprintln!(
        "building {} with {} from {}",
        artifact.package_name,
        build_mode.cargo_near_command(),
        manifest.display()
    );

    Command::new("cargo")
        .args([
            "near",
            "build",
            build_mode.cargo_near_command(),
            "--manifest-path",
        ])
        .arg(manifest)
        .current_dir(workspace_root)
        .spawn()
}

fn report_failed_status(artifact: &ArtifactMetadata, status: ExitStatus) {
    eprintln!(
        "failed {} with status {}",
        artifact.package_name,
        status_code(status)
    );
}

fn status_code(status: ExitStatus) -> String {
    status.code().map_or_else(
        || "terminated by signal".to_string(),
        |code| code.to_string(),
    )
}

#[cfg(test)]
#[path = "prebuild_test_contracts/tests.rs"]
mod tests;
