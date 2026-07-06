use std::{ffi::OsString, path::Path, process::Command};

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
fn default_build_mode_uses_reproducible_wasm() {
    assert_eq!(build_mode(false), BuildMode::Reproducible);
}

#[test]
fn debug_build_mode_uses_non_reproducible_wasm() {
    assert_eq!(build_mode(true), BuildMode::Debug);
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
