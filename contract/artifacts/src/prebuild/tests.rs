use std::{ffi::OsString, io, path::Path, process::Command};

use super::*;

#[test]
fn jobs_defaults_to_bounded_parallelism() {
    assert!((1..=DEFAULT_MAX_JOBS).contains(&default_jobs()));
}

#[test]
fn jobs_zero_normalizes_to_one() {
    assert_eq!(normalized_jobs(0), 1);
    assert_eq!(normalized_jobs(3), 3);
}

#[test]
fn catalog_is_prebuild_source_of_truth() {
    assert_eq!(artifact_catalog().len(), 14);
}

#[test]
fn artifact_selection_defaults_to_full_catalog() {
    let selected_ids = selected_artifacts(&[])
        .iter()
        .map(|artifact| artifact.id)
        .collect::<Vec<_>>();
    let catalog_ids = artifact_catalog()
        .iter()
        .map(|artifact| artifact.id)
        .collect::<Vec<_>>();

    assert_eq!(selected_ids, catalog_ids);
}

#[test]
fn artifact_selection_filters_single_artifact() {
    let artifacts = selected_artifacts(&[ArtifactId::Market]);

    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].id, ArtifactId::Market);
}

#[test]
fn artifact_selection_filters_multiple_artifacts_in_catalog_order() {
    let artifacts = selected_artifacts(&[ArtifactId::MockFt, ArtifactId::Market]);
    let ids = artifacts
        .iter()
        .map(|artifact| artifact.id)
        .collect::<Vec<_>>();

    assert_eq!(ids, vec![ArtifactId::Market, ArtifactId::MockFt]);
}

#[test]
fn artifact_selection_deduplicates_repeated_artifacts() {
    let artifacts = selected_artifacts(&[ArtifactId::Market, ArtifactId::Market]);

    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].id, ArtifactId::Market);
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
        .join(artifact.manifest_path())
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
fn wait_for_next_finished_reaps_completed_child_before_blocking() {
    let market = find_test_artifact(ArtifactId::Market);
    let mock_ft = find_test_artifact(ArtifactId::MockFt);
    let mut running = vec![
        RunningBuild {
            artifact: market,
            child: FakeChild::ready(7),
        },
        RunningBuild {
            artifact: mock_ft,
            child: FakeChild::pending(9),
        },
    ];

    let finished = wait_for_next_finished(&mut running).expect("one build should finish");

    assert_eq!(finished.artifact.id, ArtifactId::Market);
    assert_eq!(status_code(finished.status.unwrap()), "7");
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].artifact.id, ArtifactId::MockFt);
    assert_eq!(running[0].child.wait_calls, 0);
}

#[test]
fn wait_for_next_finished_blocks_on_newest_child_when_none_completed() {
    let market = find_test_artifact(ArtifactId::Market);
    let mock_ft = find_test_artifact(ArtifactId::MockFt);
    let mut running = vec![
        RunningBuild {
            artifact: market,
            child: FakeChild::pending(7),
        },
        RunningBuild {
            artifact: mock_ft,
            child: FakeChild::pending(9),
        },
    ];

    let finished = wait_for_next_finished(&mut running).expect("fallback wait should finish");

    assert_eq!(finished.artifact.id, ArtifactId::MockFt);
    assert_eq!(status_code(finished.status.unwrap()), "9");
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].artifact.id, ArtifactId::Market);
    assert_eq!(running[0].child.wait_calls, 0);
}

#[test]
fn wait_for_next_finished_reports_try_wait_errors() {
    let market = find_test_artifact(ArtifactId::Market);
    let mut running = vec![RunningBuild {
        artifact: market,
        child: FakeChild::try_error(),
    }];

    let finished = wait_for_next_finished(&mut running).expect("try_wait error should finish");

    assert_eq!(finished.artifact.id, ArtifactId::Market);
    assert!(finished.status.is_err());
    assert!(running.is_empty());
}

#[test]
fn args_accepts_jobs_flag() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--jobs"),
        OsString::from("3"),
    ]);

    assert_eq!(args.jobs, 3);
}

#[test]
fn args_accepts_zero_jobs_flag() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--jobs"),
        OsString::from("0"),
    ]);

    assert_eq!(normalized_jobs(args.jobs), 1);
}

#[test]
fn args_accepts_debug_flag() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--debug"),
    ]);

    assert!(args.debug);
}

#[test]
fn args_accepts_repeated_artifact_flags() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--artifact"),
        OsString::from("market"),
        OsString::from("--artifact"),
        OsString::from("mock-ft"),
    ]);

    assert_eq!(args.artifacts, vec![ArtifactId::Market, ArtifactId::MockFt]);
}

#[test]
fn args_accepts_comma_separated_artifact_flags() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--artifact"),
        OsString::from("market,mock-ft"),
    ]);

    assert_eq!(args.artifacts, vec![ArtifactId::Market, ArtifactId::MockFt]);
}

#[test]
fn args_rejects_unknown_artifact() {
    let error = Args::try_parse_from([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--artifact"),
        OsString::from("unknown"),
    ]);

    assert!(error.is_err());
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Args {
    match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(error) => panic!("expected args to parse: {error}"),
    }
}

fn find_test_artifact(id: ArtifactId) -> &'static ArtifactMetadata {
    crate::find_by_id(id).unwrap()
}

fn exit_status(code: i32) -> std::process::ExitStatus {
    let mut command = Command::new("sh");
    command.args(["-c", &format!("exit {code}")]);
    match command.status() {
        Ok(status) => status,
        Err(error) => panic!("expected shell command to run: {error}"),
    }
}

enum FakeTryWait {
    Pending,
    Ready(i32),
    Error,
}

struct FakeChild {
    try_wait: FakeTryWait,
    wait_code: i32,
    wait_calls: usize,
}

impl FakeChild {
    fn pending(wait_code: i32) -> Self {
        Self {
            try_wait: FakeTryWait::Pending,
            wait_code,
            wait_calls: 0,
        }
    }

    fn ready(code: i32) -> Self {
        Self {
            try_wait: FakeTryWait::Ready(code),
            wait_code: code,
            wait_calls: 0,
        }
    }

    fn try_error() -> Self {
        Self {
            try_wait: FakeTryWait::Error,
            wait_code: 1,
            wait_calls: 0,
        }
    }
}

impl BuildChild for FakeChild {
    fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        match self.try_wait {
            FakeTryWait::Pending => Ok(None),
            FakeTryWait::Ready(code) => Ok(Some(exit_status(code))),
            FakeTryWait::Error => Err(io::Error::other("try_wait failed")),
        }
    }

    fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        self.wait_calls += 1;
        Ok(exit_status(self.wait_code))
    }
}
