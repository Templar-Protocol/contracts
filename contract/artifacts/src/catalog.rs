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
/// tagged and released — that is how the `templar-gateway-testing` harness ended
/// up Tier B, and later `templar-gateway-catalog`.
///
/// Two signals, because neither covers the other. Names catch helpers that are
/// consumed (`test-utils` has dependents); the dependency graph catches ones
/// that are not, whatever they are called — which is what `catalog` needed, and
/// what a marker list could never have expressed.
#[test]
#[cfg(feature = "workspace-loader")]
#[ignore = "requires cargo metadata and workspace access"]
fn scaffolding_crates_are_excluded_from_releases() {
    const SCAFFOLDING_MARKERS: [&str; 5] = ["test", "mock", "fuzz", "harness", "fixture"];
    /// Directories whose crates ship somewhere: a WASM blob, a service, a CLI.
    /// A crate outside them that nothing consumes is scaffolding.
    const DELIVERABLE_DIRS: [&str; 4] = ["contract/", "service/", "tools/", "client/"];

    let manifest = release_plz_toml();
    let workspace_root = std::path::Path::new(env!("CARGO_WORKSPACE_DIR"));
    let metadata = crate::workspace_loader::get_metadata(workspace_root)
        .unwrap_or_else(|e| panic!("Failed to read cargo metadata: {e}"));

    let consumed = metadata
        .workspace_packages()
        .iter()
        .flat_map(|package| &package.dependencies)
        .map(|dependency| dependency.name.clone())
        .collect::<std::collections::HashSet<_>>();

    for package in metadata.workspace_packages() {
        let name = package.name.as_str();
        let relative = package
            .manifest_path
            .as_std_path()
            .strip_prefix(workspace_root)
            .unwrap_or(package.manifest_path.as_std_path())
            .to_string_lossy()
            .replace('\\', "/");

        let named_like_scaffolding = SCAFFOLDING_MARKERS
            .iter()
            .any(|marker| name.contains(marker));
        let unconsumed_library = !consumed.contains(name)
            && !DELIVERABLE_DIRS.iter().any(|dir| relative.starts_with(dir));

        let Some(reason) = (match (named_like_scaffolding, unconsumed_library) {
            (_, true) => Some("nothing in the workspace depends on it, and it ships no artifact"),
            (true, false) => Some("its name marks it as test scaffolding"),
            (false, false) => None,
        }) else {
            continue;
        };

        // A `[[package]]` block naming it, followed by `release = false` before
        // the next block starts.
        let excluded = manifest.split("[[package]]").any(|block| {
            block.contains(&format!("name = \"{name}\"")) && block.contains("release = false")
        });

        assert!(
            excluded,
            "`{name}` is internal — {reason} — but has no `release = false` block \
             in release-plz.toml, so release-plz will tag it, write it a \
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
