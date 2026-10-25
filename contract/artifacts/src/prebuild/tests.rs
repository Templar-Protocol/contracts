use std::{path::Path, process::Command};

use super::scheduler::status_code;
use super::*;
use crate::SorobanArtifactId;

mod args;
mod scheduler;

#[test]
fn artifact_selection_defaults_to_full_catalog() {
    let selected_ids = selected_artifacts(&[])
        .iter()
        .map(|artifact| artifact.id)
        .collect::<Vec<_>>();
    let catalog_ids = ArtifactId::ALL.to_vec();

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
    let artifact = ArtifactId::MockFt.metadata();
    let path = Path::new("/ws").join(artifact.manifest_path());

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

fn find_test_artifact(id: ArtifactId) -> &'static ArtifactMetadata {
    id.metadata()
}

/// What a Soroban release is published under, asserted as one value rather than
/// per-field: the catalog key the row is filed under, the asset the release
/// recipe uploads, and the `kind` that tells the workflow the NEAR recipe
/// cannot build it. Synthetic tag and version — no tag has to exist for the
/// names to be derivable, which is the whole point of resolving by package.
#[test]
fn a_soroban_tag_resolves_to_its_key_asset_and_kind() {
    let artifact = ReleaseArtifact::from_release_tag(
        "templar-proxy-oracle-soroban-pyth-lazer-source-contract-v0.1.0",
    )
    .expect("the Lazer source is catalogued");
    assert_eq!(
        resolve_lines(artifact, "0.1.0"),
        [
            "artifact=soroban-lazer-source",
            "package=templar-proxy-oracle-soroban-pyth-lazer-source-contract",
            "version=0.1.0",
            "source_path=contract/proxy-oracle/soroban/pyth-lazer-source-contract",
            "target=templar_proxy_oracle_soroban_pyth_lazer_source_contract",
            "asset=templar_proxy_oracle_soroban_pyth_lazer_source_contract-0.1.0.wasm",
            "kind=soroban",
        ]
    );
}

/// A NEAR tag's lines are exactly what they were before the Soroban stack was
/// catalogued: the workflow appends them verbatim to `$GITHUB_OUTPUT`, and an
/// absent `kind` is what its NEAR-only steps already mean by "no kind".
#[test]
fn a_near_tag_still_resolves_to_its_six_lines() {
    let artifact = ReleaseArtifact::from_release_tag("templar-market-contract-v1.5.0")
        .expect("market is catalogued");
    assert_eq!(artifact, ReleaseArtifact::Near(ArtifactId::Market));
    assert_eq!(
        resolve_lines(artifact, "1.5.0"),
        [
            "artifact=market",
            "package=templar-market-contract",
            "version=1.5.0",
            "source_path=contract/market",
            "target=templar_market_contract",
            "asset=templar_market_contract-1.5.0.wasm",
        ]
    );
}

/// Every artifact of the stack resolves to itself and to no other: package
/// names here share no prefix, and the longest-match rule is what keeps a
/// future one from quietly stealing a tag.
#[test]
fn every_soroban_tag_resolves_to_exactly_one_artifact() {
    for id in SorobanArtifactId::ALL {
        let metadata = id.metadata();
        let tag = format!("{}-v0.1.0", metadata.package_name);
        assert_eq!(
            ReleaseArtifact::from_release_tag(&tag),
            Some(ReleaseArtifact::Soroban(id)),
            "{tag} did not resolve to itself",
        );
        assert_eq!(
            ReleaseArtifact::Soroban(id).asset_name("0.1.0"),
            format!("{}-0.1.0.wasm", metadata.cargo_target_name),
        );
    }
}

/// A tag that names no contract stays "not catalogued" rather than
/// approximately matching one: `templar-proxy-oracle-soroban-common` is a
/// released library of the same stack, and a prefix match loose enough to claim
/// it would build the runtime's tag and call the bytes the runtime.
#[test]
fn a_tag_naming_nothing_resolves_to_neither_catalog() {
    for tag in [
        "templar-proxy-oracle-soroban-common-v0.2.2",
        "templar-proxy-oracle-soroban-governance-common-v0.2.2",
        "templar-proxy-oracle-soroban-con-tract-v0.1.0",
        "templar-proxy-oracle-near-contract-v0.5.1",
        "unrelated-v1.0.0",
    ] {
        let near = tag == "templar-proxy-oracle-near-contract-v0.5.1";
        assert_eq!(
            ReleaseArtifact::from_release_tag(tag).is_some(),
            near,
            "{tag} resolved the wrong way",
        );
    }
}
