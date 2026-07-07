use std::{
    collections::VecDeque,
    io,
    path::Path,
    process::{Child, ExitStatus},
    time::{Duration, Instant},
};

use crate::{workspace_loader::spawn_artifact_build, ArtifactMetadata};

const SCHEDULER_IDLE_SLEEP: Duration = Duration::from_millis(100);

pub(super) trait BuildChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
    fn wait(&mut self) -> io::Result<ExitStatus>;
    fn terminate(&mut self) -> io::Result<()>;
}

pub(super) struct BuildProcess {
    pub(super) child: Child,
}

impl BuildChild for BuildProcess {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }

    fn terminate(&mut self) -> io::Result<()> {
        terminate_process_tree(&mut self.child)
    }
}

#[cfg(unix)]
fn terminate_process_tree(child: &mut Child) -> io::Result<()> {
    let group_id = format!("-{}", child.id());
    let kill_status = std::process::Command::new("kill")
        .args(["-KILL", &group_id])
        .status();
    if matches!(kill_status, Ok(status) if status.success()) {
        return Ok(());
    }
    child.kill()
}

#[cfg(not(unix))]
fn terminate_process_tree(child: &mut Child) -> io::Result<()> {
    child.kill()
}

pub(super) struct RunningBuild<C = BuildProcess> {
    pub(super) artifact: &'static ArtifactMetadata,
    pub(super) child: C,
    pub(super) started_at: Instant,
}

pub(super) struct FinishedBuild {
    pub(super) artifact: &'static ArtifactMetadata,
    pub(super) status: io::Result<ExitStatus>,
    pub(super) timed_out: bool,
}

pub(super) fn prebuild_all(
    workspace_root: &Path,
    jobs: usize,
    timeout: Duration,
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
                workspace_root.join(artifact.manifest_path()).display()
            );

            match spawn_artifact_build(workspace_root, artifact, reproducible) {
                Ok(child) => running.push(RunningBuild {
                    artifact,
                    child: BuildProcess { child },
                    started_at: Instant::now(),
                }),
                Err(error) => {
                    eprintln!("failed to spawn {}: {error}", artifact.package_name);
                    failed = true;
                }
            }
        }

        if let Some(build) = wait_for_next_finished(&mut running, timeout) {
            match (build.timed_out, build.status) {
                (true, _) => {
                    report_timeout(build.artifact, timeout);
                    failed = true;
                }
                (false, Ok(status)) if status.success() => {
                    eprintln!("finished {}", build.artifact.package_name);
                }
                (false, Ok(status)) => {
                    report_failed_status(build.artifact, status);
                    failed = true;
                }
                (false, Err(error)) => {
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

pub(super) fn wait_for_next_finished<C: BuildChild>(
    running: &mut Vec<RunningBuild<C>>,
    timeout: Duration,
) -> Option<FinishedBuild> {
    for index in 0..running.len() {
        match running[index].child.try_wait() {
            Ok(Some(status)) => {
                let build = running.swap_remove(index);
                return Some(FinishedBuild {
                    artifact: build.artifact,
                    status: Ok(status),
                    timed_out: false,
                });
            }
            Ok(None) => {}
            Err(error) => {
                let build = running.swap_remove(index);
                return Some(FinishedBuild {
                    artifact: build.artifact,
                    status: Err(error),
                    timed_out: false,
                });
            }
        }
    }

    if let Some(index) = running
        .iter()
        .position(|build| build.started_at.elapsed() >= timeout)
    {
        let mut build = running.swap_remove(index);
        let terminate_result = build.child.terminate();
        let wait_result = build.child.wait();
        let status = terminate_result.and(wait_result);
        return Some(FinishedBuild {
            artifact: build.artifact,
            status,
            timed_out: true,
        });
    }

    std::thread::sleep(SCHEDULER_IDLE_SLEEP);
    None
}

fn report_failed_status(artifact: &ArtifactMetadata, status: ExitStatus) {
    eprintln!(
        "failed {} with status {}",
        artifact.package_name,
        status_code(status)
    );
}

fn report_timeout(artifact: &ArtifactMetadata, timeout: Duration) {
    eprintln!(
        "failed {} after timing out at {}s",
        artifact.package_name,
        timeout.as_secs()
    );
}

pub(super) fn status_code(status: ExitStatus) -> String {
    status.code().map_or_else(
        || "terminated by signal".to_string(),
        |code| code.to_string(),
    )
}
