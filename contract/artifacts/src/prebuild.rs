use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    process::{ExitCode, ExitStatus},
};

use clap::Parser;

use crate::{
    artifact_catalog, artifact_value_parser,
    workspace_loader::{spawn_artifact_build, BuildMode},
    ArtifactId, ArtifactMetadata,
};

const JOBS_ENV: &str = "PREBUILD_TEST_CONTRACTS_JOBS";
const DEFAULT_MAX_JOBS: usize = 4;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value = ".")]
    workspace_root: PathBuf,

    #[arg(long, env = JOBS_ENV, default_value_t = default_jobs())]
    jobs: usize,

    #[arg(
        long,
        help = "Use non-reproducible debug builds instead of reproducible builds"
    )]
    debug: bool,

    #[arg(
        long = "artifact",
        value_name = "ARTIFACT",
        value_parser = artifact_value_parser,
        value_delimiter = ',',
        help = "Artifact to build; repeat or separate values with commas"
    )]
    artifacts: Vec<ArtifactId>,
}

struct RunningBuild {
    artifact: &'static ArtifactMetadata,
    child: std::process::Child,
}

pub fn main() -> ExitCode {
    let args = Args::parse();
    let artifacts = selected_artifacts(&args.artifacts);

    match prebuild_all(
        &args.workspace_root,
        normalized_jobs(args.jobs),
        build_mode(args.debug),
        artifacts,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn default_jobs() -> usize {
    let available = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    available.clamp(1, DEFAULT_MAX_JOBS)
}

fn normalized_jobs(jobs: usize) -> usize {
    jobs.max(1)
}

fn build_mode(debug: bool) -> BuildMode {
    if debug {
        BuildMode::Debug
    } else {
        BuildMode::Reproducible
    }
}

fn selected_artifacts(selection: &[ArtifactId]) -> Vec<&'static ArtifactMetadata> {
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
    jobs: usize,
    build_mode: BuildMode,
    artifacts: Vec<&'static ArtifactMetadata>,
) -> Result<(), ()> {
    let mut pending = artifacts.into_iter().collect::<VecDeque<_>>();
    let mut running = Vec::<RunningBuild>::new();
    let mut failed = false;

    while !pending.is_empty() || !running.is_empty() {
        while running.len() < jobs {
            let Some(artifact) = pending.pop_front() else {
                break;
            };

            eprintln!(
                "building {} with {} from {}",
                artifact.package_name,
                build_mode.cargo_near_command(),
                workspace_root
                    .join(artifact.manifest_path())
                    .join("Cargo.toml")
                    .display()
            );

            match spawn_artifact_build(workspace_root, artifact, build_mode) {
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
mod tests;
