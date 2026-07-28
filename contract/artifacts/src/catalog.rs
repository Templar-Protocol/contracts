//! Catalog invariants that `build.rs` structurally cannot check — it validates
//! each release file's shape, but has no access to `ArtifactId` or to cargo
//! metadata. Run via `./script/check-artifact-drift.sh`.
//!
//! Source is expected to run ahead of the newest release, so these enforce only
//! that direction. Whether a release's bytes match the source is a different
//! question, answered by rebuilding at its tag in
//! `.github/workflows/release-artifacts.yml`.

use crate::ArtifactId;

/// The converse does not hold: a real contract with no releases simply has not
/// shipped yet, which is true of the NEAR vault.
#[test]
fn mocks_are_never_released() {
    for artifact in ArtifactId::ALL.iter().map(|id| id.metadata()) {
        if artifact.package_name.starts_with("mock-") {
            assert!(
                artifact.releases().is_empty(),
                "{} is a mock but lists releases",
                artifact.package_name,
            );
        }
    }
}

/// Read `release-plz.toml`.
fn release_plz_toml() -> String {
    let path = std::path::Path::new(env!("CARGO_WORKSPACE_DIR")).join("release-plz.toml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `release = true` is release-plz's default, so an unclassified crate gets
/// tagged and released — that is how the `templar-gateway-testing` harness
/// ended up Tier B. Name-based: tier is a human judgement with no mechanical
/// signal (mocks are cdylib contracts, this crate ships binaries).
#[test]
#[cfg(feature = "workspace-loader")]
#[ignore = "requires cargo metadata and workspace access"]
fn scaffolding_crates_are_excluded_from_releases() {
    const SCAFFOLDING_MARKERS: [&str; 5] = ["test", "mock", "fuzz", "harness", "fixture"];

    let manifest = release_plz_toml();
    let metadata =
        crate::workspace_loader::get_metadata(std::path::Path::new(env!("CARGO_WORKSPACE_DIR")))
            .unwrap_or_else(|e| panic!("Failed to read cargo metadata: {e}"));

    for package in metadata.workspace_packages() {
        let name = package.name.as_str();
        if !SCAFFOLDING_MARKERS
            .iter()
            .any(|marker| name.contains(marker))
        {
            continue;
        }

        // A `[[package]]` block naming it, followed by `release = false` before
        // the next block starts.
        let excluded = manifest.split("[[package]]").any(|block| {
            block.contains(&format!("name = \"{name}\"")) && block.contains("release = false")
        });

        assert!(
            excluded,
            "`{name}` looks like test scaffolding but has no `release = false` \
             block in release-plz.toml, so release-plz will tag it, write it a \
             changelog, and cut it a GitHub Release.",
        );
    }
}

/// A release must never claim a version the source has not reached.
///
/// The other direction is legitimate and common: unreleased work-in-progress is
/// *meant* to run ahead of the newest release.
#[test]
#[cfg(feature = "workspace-loader")]
#[ignore = "requires cargo metadata and workspace access"]
fn no_release_is_ahead_of_its_source() {
    use crate::workspace_loader::{find_package, get_metadata};

    let workspace_dir = std::path::Path::new(env!("CARGO_WORKSPACE_DIR"));
    let metadata = get_metadata(workspace_dir)
        .unwrap_or_else(|e| panic!("Failed to read cargo metadata: {e}"));

    for artifact in ArtifactId::ALL.iter().map(|id| id.metadata()) {
        let Some(newest) = artifact.version() else {
            continue;
        };
        let package = find_package(&metadata, artifact.package_name).unwrap_or_else(|| {
            panic!(
                "Package '{}' not found in workspace metadata — catalog may be stale.",
                artifact.package_name,
            )
        });
        let source = &package.version;
        let newest_semver =
            cargo_metadata::semver::Version::parse(newest).unwrap_or_else(|error| {
                panic!(
                    "{} has a catalogued release {newest} that is not valid semver: {error}",
                    artifact.package_name,
                )
            });

        assert!(
            newest_semver <= *source,
            "{} has a catalogued release {} but its Cargo.toml is only at {}. \
             A release cannot exist for a version the source never reached — \
             either the entry is wrong, or the version was reverted after \
             shipping.",
            artifact.package_name,
            newest,
            source,
        );
    }
}
