//! Catalog invariants that `build.rs` structurally cannot check — it validates
//! each release file's shape, but has no access to `ArtifactId` or to cargo
//! metadata. Run via `./script/check-artifact-drift.sh`.
//!
//! Source is expected to run ahead of the newest release, so these enforce only
//! that direction. Whether a release's bytes match the source is a different
//! question, answered by rebuilding at its tag in
//! `.github/workflows/release-artifacts.yml`.

use crate::ArtifactId;

/// The converse does not hold: a real contract with no releases simply has no
/// version anyone has chosen to cut a WASM for yet.
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

/// `current()` is the last entry of `releases()`, so it names the newest
/// release only while each list ascends — an order `build.rs` imposes rather
/// than inherits from `read_dir`. By version, not by date: a 3.1.0 backported
/// after 4.0.0 sorts before it, leaving `current()` at 4.0.0.
#[test]
fn releases_ascend_by_version() {
    for artifact in ArtifactId::ALL.iter().map(|id| id.metadata()) {
        let versions = artifact
            .releases()
            .iter()
            .map(|release| {
                // `build.rs` rejects any spelling this cannot parse.
                semver::Version::parse(release.version).unwrap_or_else(|error| {
                    panic!(
                        "{}: release {} is not valid semver: {error}",
                        artifact.package_name, release.version,
                    )
                })
            })
            .collect::<Vec<_>>();

        assert!(
            versions.is_sorted_by(|a, b| a < b),
            "{}'s releases are not in strictly ascending version order: {:?}. \
             `current()` would not be the newest release.",
            artifact.package_name,
            versions.iter().map(ToString::to_string).collect::<Vec<_>>(),
        );
    }
}

/// Parsed `release-plz.toml`.
fn release_plz_config() -> toml::Value {
    let path = std::path::Path::new(env!("CARGO_WORKSPACE_DIR")).join("release-plz.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Nothing reaches a registry unless this is flipped.
///
/// Tier A and B crates are publishable in their own `Cargo.toml` so release-plz
/// will version and tag them at all — which means this one line, rather than 52
/// manifests, is what stands between a release and an upload.
#[test]
fn no_tier_can_reach_a_registry() {
    let publish = release_plz_config()
        .get("workspace")
        .and_then(|workspace| workspace.get("publish"))
        .and_then(toml::Value::as_bool);

    assert_eq!(
        publish,
        Some(false),
        "release-plz.toml must set `[workspace] publish = false`. Without it \
         every Tier A and B crate is uploaded on release, and crates.io \
         publishing is deliberately deferred — see RELEASING.md.",
    );
}

/// Packages `release-plz.toml` marks `release = false`.
///
/// Parsed, not searched: a commented-out `# release = false` satisfies a
/// substring check while release-plz applies its `release = true` default.
fn release_false_packages() -> std::collections::BTreeSet<String> {
    release_plz_config()
        .get("package")
        .and_then(toml::Value::as_array)
        .map(|packages| {
            packages
                .iter()
                .filter(|package| {
                    package.get("release").and_then(toml::Value::as_bool) == Some(false)
                })
                .filter_map(|package| package.get("name").and_then(toml::Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// `release = true` is release-plz's default, so an unclassified crate is
/// tagged, changelogged and released.
///
/// Tier C is derived, not listed: RELEASING.md makes `publish = false` in a
/// crate's own manifest the defining property, so the workspace already states
/// the set. A literal copy here, or a name/path heuristic guessing at new
/// crates, would each be one more place to drift.
#[test]
#[cfg(feature = "workspace-loader")]
#[ignore = "requires cargo metadata and workspace access"]
fn internal_crates_are_excluded_from_releases() {
    let metadata =
        crate::workspace_loader::get_metadata(std::path::Path::new(env!("CARGO_WORKSPACE_DIR")))
            .unwrap_or_else(|e| panic!("Failed to read cargo metadata: {e}"));

    // `publish = false` parses as an empty allow-list of registries.
    let internal = metadata
        .workspace_packages()
        .iter()
        .filter(|package| package.publish.as_deref() == Some(&[]))
        .map(|package| package.name.to_string())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(
        release_false_packages(),
        internal,
        "release-plz.toml's `release = false` set must be exactly the crates \
         whose Cargo.toml forbids publishing. A crate missing from it is tagged, \
         changelogged and released by default; a crate wrongly in it is a \
         deliverable that will never be tagged.",
    );
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

/// Total and unambiguous: every release is reachable from its own digest, and no two share one.
///
/// The lookup is how a caller holding only a hash — read off a registry, say — reaches
/// `released_bytes`, so a digest that resolved to a different artifact would fetch the wrong wasm.
#[test]
fn every_release_resolves_from_its_digest() {
    for artifact in ArtifactId::ALL {
        for release in artifact.metadata().releases() {
            let (found_artifact, found) =
                crate::release_by_sha256(release.sha256).unwrap_or_else(|| {
                    panic!(
                        "{}@{} is catalogued but its digest {} resolves to nothing",
                        artifact.metadata().package_name,
                        release.version,
                        release.sha256,
                    )
                });

            assert_eq!(
                (found_artifact, found.version),
                (artifact, release.version),
                "digest {} is shared by {}@{} and {}@{}",
                release.sha256,
                artifact.metadata().package_name,
                release.version,
                found_artifact.metadata().package_name,
                found.version,
            );
        }
    }
}

#[test]
fn digest_lookup_ignores_hex_case_and_rejects_strangers() {
    let release = ArtifactId::Registry
        .metadata()
        .release("1.2.4")
        .expect("registry 1.2.4 is catalogued");

    assert_eq!(
        crate::release_by_sha256(&release.sha256.to_uppercase()).map(|(id, _)| id),
        Some(ArtifactId::Registry),
    );
    assert_eq!(crate::release_by_sha256(&"0".repeat(64)), None);
}
