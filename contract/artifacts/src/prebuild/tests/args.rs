use std::{ffi::OsString, path::PathBuf};

use super::super::*;

#[test]
fn accepts_jobs_flag() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--jobs"),
        OsString::from("3"),
    ]);

    assert_eq!(args.jobs, 3);
}

#[test]
fn accepts_zero_jobs_flag() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--jobs"),
        OsString::from("0"),
    ]);

    assert_eq!(args.jobs.max(1), 1);
}

#[test]
fn accepts_timeout_secs_flag() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--timeout-secs"),
        OsString::from("42"),
    ]);

    assert_eq!(args.timeout_secs, 42);
}

#[test]
fn defaults_to_drift_profile() {
    let args = parse_args([OsString::from("prebuild-test-contracts")]);

    assert_eq!(args.profile, PrebuildProfile::Drift);
    assert!(args.profile.reproducible());
}

#[test]
fn accepts_test_profile() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--profile"),
        OsString::from("test"),
    ]);

    assert_eq!(args.profile, PrebuildProfile::Test);
    assert!(!args.profile.reproducible());
}

#[test]
fn accepts_drift_profile() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--profile"),
        OsString::from("drift"),
    ]);

    assert_eq!(args.profile, PrebuildProfile::Drift);
    assert!(args.profile.reproducible());
}

#[test]
fn rejects_debug_flag() {
    let error = Args::try_parse_from([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--debug"),
    ]);

    assert!(error.is_err());
}

#[test]
fn accepts_repeated_artifact_flags() {
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
fn accepts_comma_separated_artifact_flags() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--artifact"),
        OsString::from("market,mock-ft"),
    ]);

    assert_eq!(args.artifacts, vec![ArtifactId::Market, ArtifactId::MockFt]);
}

#[test]
fn accepts_artifact_package_name_alias() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--artifact"),
        OsString::from("templar-market-contract"),
    ]);

    assert_eq!(args.artifacts, vec![ArtifactId::Market]);
}

#[test]
fn workspace_root_defaults_to_discovered_workspace() {
    let args = parse_args([OsString::from("prebuild-test-contracts")]);

    assert_eq!(args.workspace_root, default_workspace_root());
}

#[test]
fn accepts_workspace_root_flag() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--workspace-root"),
        OsString::from("/some/path"),
    ]);

    assert_eq!(args.workspace_root, PathBuf::from("/some/path"));
}

#[test]
fn rejects_unknown_artifact() {
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
