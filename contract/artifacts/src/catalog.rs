//! Catalog invariants.
//!
//! Pure, in-memory checks — no contract builds, no Docker, seconds — run via
//! `./script/check-artifact-drift.sh`.
//!
//! The catalog records what was **released**, which is not the same as what the
//! crate versions say. Contract versions have historically been bumped during
//! development without ever being deployed (market reached 1.4.0 on a 1.3.0
//! release, registry 1.2.1 on a 1.1.0 release), so source is expected to run
//! ahead of the newest release. The checks below enforce that direction and
//! nothing stronger.
//!
//! Whether a release's bytes match what the source compiles to is a different
//! question, answered by rebuilding at its tag — see
//! `.github/workflows/release-artifacts.yml`.

use std::collections::HashSet;

use crate::ArtifactId;

/// `"1.2.1"` -> `(1, 2, 1)`, for ordering comparisons.
fn parts(version: &str) -> (u64, u64, u64) {
    let mut it = version.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// Structural invariants of each artifact's release list.
#[test]
fn catalog_releases_are_well_formed() {
    for artifact in ArtifactId::ALL.iter().map(|id| id.metadata()) {
        let mut seen = HashSet::new();
        let mut previous: Option<&str> = None;

        for release in artifact.releases() {
            assert!(
                seen.insert(release.version),
                "{} lists version {} more than once — two different builds cannot \
                 both be that version",
                artifact.package_name,
                release.version,
            );
            assert_eq!(
                release.sha256.len(),
                64,
                "{}@{} sha256 is not a 64-char hex digest",
                artifact.package_name,
                release.version,
            );
            assert!(
                release.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "{}@{} sha256 is not hex",
                artifact.package_name,
                release.version,
            );
            // Recorded, not derived — so their absence is a real possibility
            // worth asserting rather than a shape the code guarantees.
            assert!(
                !release.tag.is_empty() && !release.asset.is_empty(),
                "{}@{} is missing its tag or asset; the download URL is built \
                 from both",
                artifact.package_name,
                release.version,
            );

            // `current()` is defined as "the last entry", and the rest of the
            // crate relies on that meaning "newest". CI appends, so the only way
            // to break this is a hand-edit or an out-of-order backport.
            if let Some(previous) = previous {
                assert!(
                    parts(previous) < parts(release.version),
                    "{}: release {} is listed after {}, but releases must be \
                     oldest first",
                    artifact.package_name,
                    release.version,
                    previous,
                );
            }
            previous = Some(release.version);
        }
    }
}

/// Mocks are test scaffolding — never tagged, never deployed — so they can
/// never acquire a release. (The converse does not hold: a real contract with
/// no releases simply has not shipped yet, which is true of the NEAR vault.)
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

/// Read `release-plz.toml`. It is the producer of every release tag, so several
/// invariants here are really "does our code still agree with that file".
fn release_plz_toml() -> String {
    let path = std::path::Path::new(env!("CARGO_WORKSPACE_DIR")).join("release-plz.toml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Test scaffolding must never be released.
///
/// `release = true` is release-plz's default, so a crate nobody classified gets
/// tagged, changelogged, and given a GitHub Release. That is how
/// `templar-gateway-testing` — a sandbox harness — ended up Tier B.
///
/// Name-based, and deliberately so: tier is a human judgement with no clean
/// mechanical signal (mocks are cdylib contracts, `templar-contract-artifacts`
/// ships binaries). This catches the crates whose names announce what they are,
/// which is the case that has actually bitten us.
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
        // `package.version` is already a `semver::Version`; parse the catalog
        // side to match rather than stringifying a typed value and comparing
        // both through the lossy `parts` fallback.
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
