use std::{
    collections::{HashSet, VecDeque},
    io,
    path::{Path, PathBuf},
    process::{ExitCode, ExitStatus, Output},
    thread::JoinHandle,
};

use clap::{Parser, ValueEnum};

use crate::{
    artifact_catalog, artifact_value_parser, workspace_loader,
    workspace_loader::spawn_artifact_build, ArtifactId, ArtifactMetadata,
};

const DEFAULT_MAX_JOBS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum PrebuildProfile {
    Test,
    Drift,
}

impl PrebuildProfile {
    const fn reproducible(self) -> bool {
        match self {
            Self::Test => false,
            Self::Drift => true,
        }
    }
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value_os_t = default_workspace_root())]
    workspace_root: PathBuf,

    #[arg(long, env = "PREBUILD_TEST_CONTRACTS_JOBS", default_value_t = default_jobs())]
    jobs: usize,

    /// Build profile for contract artifacts.
    #[arg(long, value_enum, default_value_t = PrebuildProfile::Drift)]
    profile: PrebuildProfile,

    /// Artifact to build; repeat or separate values with commas.
    #[arg(
        long = "artifact",
        value_name = "ARTIFACT",
        value_parser = artifact_value_parser,
        value_delimiter = ','
    )]
    artifacts: Vec<ArtifactId>,
}

trait BuildChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
    fn wait(&mut self) -> io::Result<ExitStatus>;
    fn take_output(&mut self) -> Option<std::process::Output> {
        None
    }
}

struct BuildHandle {
    handle: Option<JoinHandle<io::Result<Output>>>,
    captured: Option<Output>,
}

impl BuildChild for BuildHandle {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let Some(handle) = &self.handle else {
            return Ok(None);
        };
        if !handle.is_finished() {
            return Ok(None);
        }
        self.collect_output().map(Some)
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        self.collect_output()
    }

    fn take_output(&mut self) -> Option<Output> {
        self.captured.take()
    }
}

impl BuildHandle {
    fn collect_output(&mut self) -> io::Result<ExitStatus> {
        let Some(handle) = self.handle.take() else {
            return Err(io::Error::other("build output was already consumed"));
        };
        let output = join_build_thread(handle)?;
        let status = output.status;
        self.captured = Some(output);
        Ok(status)
    }
}

fn join_build_thread(handle: JoinHandle<io::Result<Output>>) -> io::Result<Output> {
    handle
        .join()
        .map_err(|_| io::Error::other("build thread panicked"))?
}

struct RunningBuild<C = BuildHandle> {
    artifact: &'static ArtifactMetadata,
    child: C,
}

struct FinishedBuild {
    artifact: &'static ArtifactMetadata,
    status: io::Result<ExitStatus>,
    captured_output: Option<Output>,
}

pub fn main() -> ExitCode {
    let args = Args::parse();
    let jobs = args.jobs.max(1);
    let reproducible = args.profile.reproducible();
    let artifacts = selected_artifacts(&args.artifacts);

    match prebuild_all(&args.workspace_root, jobs, reproducible, artifacts) {
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

fn default_workspace_root() -> PathBuf {
    workspace_loader::discover_workspace_root().unwrap_or_else(|| PathBuf::from("."))
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
    reproducible: bool,
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
                "building {} from {}",
                artifact.package_name,
                workspace_root
                    .join(artifact.manifest_path())
                    .join("Cargo.toml")
                    .display()
            );

            let handle = spawn_artifact_build(workspace_root, artifact, reproducible);
            running.push(RunningBuild {
                artifact,
                child: BuildHandle {
                    handle: Some(handle),
                    captured: None,
                },
            });
        }

        if let Some(build) = wait_for_next_finished(&mut running) {
            print_build_output(&build);
            match build.status {
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

fn wait_for_next_finished<C: BuildChild>(
    running: &mut Vec<RunningBuild<C>>,
) -> Option<FinishedBuild> {
    for index in 0..running.len() {
        match running[index].child.try_wait() {
            Ok(Some(status)) => {
                let mut build = running.swap_remove(index);
                let captured_output = build.child.take_output();
                return Some(FinishedBuild {
                    artifact: build.artifact,
                    status: Ok(status),
                    captured_output,
                });
            }
            Ok(None) => {}
            Err(error) => {
                let mut build = running.swap_remove(index);
                let captured_output = build.child.take_output();
                return Some(FinishedBuild {
                    artifact: build.artifact,
                    status: Err(error),
                    captured_output,
                });
            }
        }
    }

    running.pop().map(|mut build| FinishedBuild {
        artifact: build.artifact,
        status: build.child.wait(),
        captured_output: build.child.take_output(),
    })
}

fn print_build_output(build: &FinishedBuild) {
    let Some(output) = &build.captured_output else {
        return;
    };
    let artifact_name = build.artifact.package_name;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.trim().is_empty() {
        eprintln!("=== stdout for {artifact_name} ===");
        eprint!("{stdout}");
    }
    if !stderr.trim().is_empty() {
        eprintln!("=== stderr for {artifact_name} ===");
        eprint!("{stderr}");
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
