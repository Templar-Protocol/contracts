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
fn default_build_mode_uses_reproducible_wasm() {
    assert_eq!(
        BuildMode::from_debug(false).cargo_near_command(),
        "reproducible-wasm"
    );
}

#[test]
fn debug_build_mode_uses_non_reproducible_wasm() {
    assert_eq!(
        BuildMode::from_debug(true).cargo_near_command(),
        "non-reproducible-wasm"
    );
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
    let artifacts = selected_artifacts(&[ContractArtifact::Market]);

    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].id, ContractArtifact::Market);
}

#[test]
fn artifact_selection_filters_multiple_artifacts_in_catalog_order() {
    let artifacts = selected_artifacts(&[ContractArtifact::MockFt, ContractArtifact::Market]);
    let ids = artifacts
        .iter()
        .map(|artifact| artifact.id)
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        vec![ContractArtifact::Market, ContractArtifact::MockFt]
    );
}

#[test]
fn artifact_selection_deduplicates_repeated_artifacts() {
    let artifacts = selected_artifacts(&[ContractArtifact::Market, ContractArtifact::Market]);

    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].id, ContractArtifact::Market);
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
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--jobs"),
        OsString::from("3"),
    ]);

    let Some(jobs) = args.jobs else {
        panic!("--jobs should populate jobs");
    };

    assert_eq!(jobs.get(), 3);
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

    assert_eq!(
        args.artifacts,
        vec![ContractArtifact::Market, ContractArtifact::MockFt]
    );
}

#[test]
fn args_accepts_comma_separated_artifact_flags() {
    let args = parse_args([
        OsString::from("prebuild-test-contracts"),
        OsString::from("--artifact"),
        OsString::from("market,mock-ft"),
    ]);

    assert_eq!(
        args.artifacts,
        vec![ContractArtifact::Market, ContractArtifact::MockFt]
    );
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
