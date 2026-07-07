use std::{
    io,
    process::Command,
    time::{Duration, Instant},
};

use crate::ArtifactId;

use super::find_test_artifact;
use crate::prebuild::scheduler::{status_code, wait_for_next_finished, BuildChild, RunningBuild};

#[test]
fn wait_for_next_finished_reaps_completed_child_before_blocking() {
    let market = find_test_artifact(ArtifactId::Market);
    let mock_ft = find_test_artifact(ArtifactId::MockFt);
    let mut running = vec![
        RunningBuild {
            artifact: market,
            child: FakeChild::ready(7),
            started_at: Instant::now(),
        },
        RunningBuild {
            artifact: mock_ft,
            child: FakeChild::pending(9),
            started_at: Instant::now(),
        },
    ];

    let finished = wait_for_next_finished(&mut running, Duration::from_secs(30))
        .expect("one build should finish");

    assert_eq!(finished.artifact.id, ArtifactId::Market);
    assert_eq!(status_code(finished.status.unwrap()), "7");
    assert!(!finished.timed_out);
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].artifact.id, ArtifactId::MockFt);
    assert_eq!(running[0].child.wait_calls, 0);
}

#[test]
fn wait_for_next_finished_returns_none_when_pending_before_timeout() {
    let market = find_test_artifact(ArtifactId::Market);
    let mock_ft = find_test_artifact(ArtifactId::MockFt);
    let mut running = vec![
        RunningBuild {
            artifact: market,
            child: FakeChild::pending(7),
            started_at: Instant::now(),
        },
        RunningBuild {
            artifact: mock_ft,
            child: FakeChild::pending(9),
            started_at: Instant::now(),
        },
    ];

    let finished = wait_for_next_finished(&mut running, Duration::from_secs(30));

    assert!(finished.is_none());
    assert_eq!(running.len(), 2);
    assert_eq!(running[0].artifact.id, ArtifactId::Market);
    assert_eq!(running[0].child.wait_calls, 0);
}

#[test]
fn wait_for_next_finished_reports_try_wait_errors() {
    let market = find_test_artifact(ArtifactId::Market);
    let mut running = vec![RunningBuild {
        artifact: market,
        child: FakeChild::try_error(),
        started_at: Instant::now(),
    }];

    let finished = wait_for_next_finished(&mut running, Duration::from_secs(30))
        .expect("try_wait error should finish");

    assert_eq!(finished.artifact.id, ArtifactId::Market);
    assert!(finished.status.is_err());
    assert!(!finished.timed_out);
    assert!(running.is_empty());
}

#[test]
fn wait_for_next_finished_terminates_build_after_timeout() {
    let market = find_test_artifact(ArtifactId::Market);
    let started_at = Instant::now()
        .checked_sub(Duration::from_secs(31))
        .expect("test duration should be representable");
    let mut running = vec![RunningBuild {
        artifact: market,
        child: FakeChild::pending(9),
        started_at,
    }];

    let finished = wait_for_next_finished(&mut running, Duration::from_secs(30))
        .expect("timed-out build should finish");

    assert_eq!(finished.artifact.id, ArtifactId::Market);
    assert!(finished.timed_out);
    assert_eq!(status_code(finished.status.unwrap()), "9");
    assert!(running.is_empty());
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
    terminated: bool,
}

impl FakeChild {
    fn pending(wait_code: i32) -> Self {
        Self {
            try_wait: FakeTryWait::Pending,
            wait_code,
            wait_calls: 0,
            terminated: false,
        }
    }

    fn ready(code: i32) -> Self {
        Self {
            try_wait: FakeTryWait::Ready(code),
            wait_code: code,
            wait_calls: 0,
            terminated: false,
        }
    }

    fn try_error() -> Self {
        Self {
            try_wait: FakeTryWait::Error,
            wait_code: 1,
            wait_calls: 0,
            terminated: false,
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
        if !self.terminated {
            return Err(io::Error::other("wait called before terminate"));
        }
        Ok(exit_status(self.wait_code))
    }

    fn terminate(&mut self) -> io::Result<()> {
        self.terminated = true;
        Ok(())
    }
}
