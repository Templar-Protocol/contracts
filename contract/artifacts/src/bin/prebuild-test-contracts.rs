use std::{
    collections::VecDeque,
    env,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::{Command, ExitCode, ExitStatus},
};

use clap::Parser;
use templar_contract_artifacts::{artifact_catalog, manifest_path, ArtifactMetadata};

const JOBS_ENV: &str = "PREBUILD_TEST_CONTRACTS_JOBS";
const DEFAULT_MAX_JOBS: usize = 4;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value = ".")]
    workspace_root: PathBuf,

    #[arg(long, value_parser = parse_jobs)]
    jobs: Option<NonZeroUsize>,
}

struct RunningBuild {
    artifact: &'static ArtifactMetadata,
    child: std::process::Child,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let jobs = args
        .jobs
        .or_else(jobs_from_env)
        .unwrap_or_else(default_jobs);

    match prebuild_all(&args.workspace_root, jobs) {
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

fn prebuild_all(workspace_root: &Path, jobs: NonZeroUsize) -> Result<(), ()> {
    let mut pending = artifact_catalog().iter().collect::<VecDeque<_>>();
    let mut running = Vec::<RunningBuild>::new();
    let mut failed = false;

    while !pending.is_empty() || !running.is_empty() {
        while running.len() < jobs.get() {
            let Some(artifact) = pending.pop_front() else {
                break;
            };

            match spawn_build(workspace_root, artifact) {
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
) -> std::io::Result<std::process::Child> {
    let manifest = workspace_root
        .join(manifest_path(artifact))
        .join("Cargo.toml");

    eprintln!(
        "building {} from {}",
        artifact.package_name,
        manifest.display()
    );

    Command::new("cargo")
        .args(["near", "build", "reproducible-wasm", "--manifest-path"])
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
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn parse_jobs_rejects_zero() {
        assert!(parse_jobs("0").is_err());
    }

    #[test]
    fn parse_jobs_accepts_positive_integer() {
        let jobs = match parse_jobs("2") {
            Ok(jobs) => jobs,
            Err(error) => panic!("expected valid jobs value: {error}"),
        };

        assert_eq!(jobs.get(), 2);
    }

    #[test]
    fn default_jobs_is_bounded_and_nonzero() {
        let jobs = default_jobs().get();
        assert!((1..=DEFAULT_MAX_JOBS).contains(&jobs));
    }

    #[test]
    fn catalog_is_prebuild_source_of_truth() {
        assert_eq!(artifact_catalog().len(), 14);
    }

    #[test]
    fn manifest_path_uses_catalog_source_path() {
        let Some(artifact) = artifact_catalog()
            .iter()
            .find(|artifact| artifact.package_name == "mock-ft")
        else {
            panic!("mock-ft artifact should be present in catalog");
        };
        let path = Path::new("/ws")
            .join(manifest_path(artifact))
            .join("Cargo.toml");

        assert_eq!(path, Path::new("/ws/mock/ft/Cargo.toml"));
    }

    #[test]
    fn status_code_formats_exit_code() {
        let mut command = Command::new("sh");
        command.args(["-c", "exit 7"]);
        let status = match command.status() {
            Ok(status) => status,
            Err(error) => panic!("expected shell command to run: {error}"),
        };

        assert_eq!(status_code(status), "7");
    }

    #[test]
    fn args_accepts_jobs_flag() {
        let args = match Args::try_parse_from([
            OsString::from("prebuild-test-contracts"),
            OsString::from("--jobs"),
            OsString::from("3"),
        ]) {
            Ok(args) => args,
            Err(error) => panic!("expected args to parse: {error}"),
        };

        let Some(jobs) = args.jobs else {
            panic!("--jobs should populate jobs");
        };

        assert_eq!(jobs.get(), 3);
    }
}
