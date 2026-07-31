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

/// `release = true` is release-plz's default, so an unclassified crate gets
/// tagged and released — that is how the `templar-gateway-testing` harness ended
/// up Tier B, and later `templar-gateway-catalog`.
///
/// Two layers. The authoritative Tier C set is asserted exactly, so a crate
/// cannot drop out of it by a typo, a rename, or a commented-out line. The
/// classification below is the net for a *newly added* crate that nobody
/// thought to classify — it flags candidates the exact set would not know to
/// expect.
#[test]
#[cfg(feature = "workspace-loader")]
#[ignore = "requires cargo metadata and workspace access"]
fn scaffolding_crates_are_excluded_from_releases() {
    /// Tier C, as documented in RELEASING.md. Editing this list is the
    /// deliberate act of reclassifying a crate.
    const TIER_C: [&str; 11] = [
        "mock-ft",
        "mock-mt",
        "mock-oracle",
        "mock-receiver",
        "mock-ref",
        "templar-contract-artifacts",
        "templar-fuzz",
        "templar-gateway-catalog",
        "templar-gateway-testing",
        "templar-proxy-oracle-soroban-integration-tests",
        "test-utils",
    ];
    const SCAFFOLDING_MARKERS: [&str; 5] = ["test", "mock", "fuzz", "harness", "fixture"];
    /// Directories whose crates ship somewhere: a WASM blob, a service, a CLI.
    /// A crate outside them that nothing consumes is scaffolding.
    const DELIVERABLE_DIRS: [&str; 4] = ["contract/", "service/", "tools/", "client/"];

    let excluded = release_false_packages();
    let expected = TIER_C
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        excluded, expected,
        "release-plz.toml's `release = false` set has drifted from Tier C. \
         Anything missing gets tagged, changelogged and released by default."
    );

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

        assert!(
            excluded.contains(name),
            "`{name}` is internal — {reason} — but is not in release-plz.toml's \
             `release = false` set, so release-plz will tag it, write it a \
             changelog, and cut it a GitHub Release. Add it there and to TIER_C.",
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
