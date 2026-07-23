//! Compile-time WASM blobs via `include_bytes!`.
//!
//! Enabled via the `embedded-wasm` feature.
//!
//! # Where the bytes live
//!
//! One directory per released version:
//! `contract/artifacts/res/near/{target_name}/{version}/{target_name}.wasm`.
//! These files are the **single source of truth** for the embedded bytes.
//!
//! Releases are **immutable**. Shipping new bytes means bumping the contract's
//! crate version and *adding* a release, never editing an existing one —
//! historical blobs are what migration and upgrade tests deploy, so rewriting
//! one silently invalidates those tests. Cut a release with:
//! ```bash
//! just artifact-release <artifact-id>
//! ```
//! which builds reproducibly from the committed tree, installs the blob, and
//! prints the catalog entry to add to `ids.rs`.
//!
//! # Consistency guarantees
//!
//! Source is allowed to move ahead of the newest released blob; unreleased
//! work-in-progress is *meant* to lag it. The checks below are pure and
//! in-memory (no rebuild), and run via `./script/check-artifact-drift.sh`:
//!
//! - [`embedded_drift_check`] — every release's blob hashes to its pinned
//!   `sha256`, so bytes and pin must change together in one reviewable diff.
//! - [`embedded_version_drift_check`] — the newest release matches the crate's
//!   `Cargo.toml` version. This is the tripwire: bumping the version fails the
//!   check until the matching blob is cut.
//! - [`catalog_releases_have_embedded_bytes`] / [`catalog_releases_are_well_formed`]
//!   / [`catalog_matches_disk`] — the catalog, the byte-loading arms, and the
//!   files on disk all agree.
//!
//! Whether the bytes actually match what the source compiles to is verified
//! separately, on release tags, by `.github/workflows/release-artifacts.yml`.

use crate::ArtifactId;

/// Return the size in bytes of the embedded WASM for every catalogued artifact.
pub fn embedded_sizes() -> Vec<(&'static str, usize)> {
    ArtifactId::ALL
        .iter()
        .map(|id| (id.as_str(), id.embedded_bytes().len()))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// All 14 catalogued artifacts must return non-empty WASM starting with
    /// the magic bytes. This test fails at compile time if a checked-in file
    /// is missing, and at runtime if a blob is corrupt or empty.
    #[test]
    fn test_read_embedded_all_artifacts() {
        for artifact in ArtifactId::ALL.iter().map(|id| id.metadata()) {
            let bytes = artifact.id.embedded_bytes();
            assert!(
                !bytes.is_empty(),
                "{} has empty embedded WASM",
                artifact.package_name,
            );
            assert_eq!(
                &bytes[0..4],
                b"\0asm",
                "{} embedded blob does not start with WASM magic bytes",
                artifact.package_name,
            );
        }
    }

    #[test]
    fn test_embedded_sizes_all_artifacts() {
        let sizes = embedded_sizes();
        let expected = ArtifactId::ALL
            .iter()
            .map(|id| (id.as_str(), id.embedded_bytes().len()))
            .collect::<Vec<_>>();

        assert_eq!(sizes, expected);
        for (name, size) in &sizes {
            assert!(*size > 0, "embedded size for {name} is zero");
        }
    }

    /// **Blob hash-pin check** — verifies every checked-in embedded blob hashes
    /// to the `expected_sha256` pinned in its catalog entry.
    ///
    /// This makes a blob change a reviewable edit: the binary and its pinned
    /// hash must change together in the same diff, or this fails. It is a pure,
    /// in-memory comparison — no rebuild, no `target/near`, no reproducible
    /// toolchain — so it runs in the normal test suite.
    ///
    /// The embedded blobs are treated as versioned, pinned release artifacts,
    /// not a mirror of `HEAD`: source is free to move ahead of a shipped blob.
    /// To deliberately ship new bytes for an artifact:
    /// 1. `cargo near build reproducible-wasm --manifest-path <source_path>/Cargo.toml`
    /// 2. `cp target/near/<contract>/<contract>.wasm contract/artifacts/res/near/<contract>/<contract>.wasm`
    /// 3. Update that entry's `expected_sha256` (and `version`) in `ids.rs`.
    #[test]
    fn embedded_drift_check() {
        use crate::sha256_hex;

        // Report every mismatch at once rather than panicking on the first, so a
        // batch refresh is a single edit pass instead of fix-one-then-rerun.
        // Checks *every* release, not just the newest: historical blobs are
        // immutable, so a change to one is always a mistake.
        let drifted = ArtifactId::ALL
            .iter()
            .map(|id| id.metadata())
            .flat_map(|artifact| {
                artifact.releases.iter().filter_map(move |release| {
                    let bytes = artifact.id.embedded_bytes_for_version(release.version)?;
                    let actual = sha256_hex(bytes);
                    (actual != release.sha256).then(|| {
                        format!(
                            "  {}@{} — blob is {actual}, catalog pins {}",
                            artifact.package_name, release.version, release.sha256,
                        )
                    })
                })
            })
            .collect::<Vec<_>>();

        assert!(
            drifted.is_empty(),
            "Blob hash drift for {} release(s) — checked-in bytes do not match the \
             `sha256` pinned in ids.rs:\n{}\n\
             Released blobs are immutable. If you meant to ship new bytes, add a \
             NEW release entry (`just artifact-release <id>`) rather than editing \
             an existing one.",
            drifted.len(),
            drifted.join("\n"),
        );
    }

    /// Every catalogued release must have an `include_bytes!` arm in
    /// `embedded_bytes_for_version`, and the newest release must be loadable via
    /// `embedded_bytes`. Catches a release added to the catalog table without a
    /// matching byte-loading arm (or vice versa).
    #[test]
    fn catalog_releases_have_embedded_bytes() {
        let missing = ArtifactId::ALL
            .iter()
            .map(|id| id.metadata())
            .flat_map(|artifact| {
                artifact
                    .releases
                    .iter()
                    .filter(move |release| {
                        artifact
                            .id
                            .embedded_bytes_for_version(release.version)
                            .is_none()
                    })
                    .map(move |release| format!("  {}@{}", artifact.package_name, release.version))
            })
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "{} catalogued release(s) have no embedded bytes — add an arm to \
             `ArtifactId::embedded_bytes_for_version`:\n{}",
            missing.len(),
            missing.join("\n"),
        );

        // An unknown version must be `None`, not a panic or the wrong blob.
        assert!(ArtifactId::Market
            .embedded_bytes_for_version("0.0.0-nonexistent")
            .is_none());
    }

    /// Structural invariants of each artifact's release list.
    #[test]
    fn catalog_releases_are_well_formed() {
        for artifact in ArtifactId::ALL.iter().map(|id| id.metadata()) {
            assert!(
                !artifact.releases.is_empty(),
                "{} has no releases; every artifact needs at least one set of \
                 deployable bytes",
                artifact.package_name,
            );

            let mut seen = std::collections::HashSet::new();
            for release in artifact.releases {
                assert!(
                    seen.insert(release.version),
                    "{} lists version {} more than once — two different blobs \
                     cannot both be that version",
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
            }

            // `current()` is defined as "newest", and the rest of the crate
            // relies on that being the last entry.
            assert_eq!(
                artifact.current().version,
                artifact
                    .releases
                    .last()
                    .expect("non-empty checked above")
                    .version,
            );
        }
    }

    /// Every `res/near/**` blob on disk must be catalogued, and every catalogued
    /// release must exist on disk. Catches an orphaned directory left behind by
    /// a rename, and a catalog entry pointing at bytes nobody committed.
    #[test]
    fn catalog_matches_disk() {
        let res = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("res/near");

        let mut on_disk = std::collections::BTreeSet::new();
        for target in std::fs::read_dir(&res).expect("res/near is missing") {
            let target = target.expect("unreadable res/near entry").path();
            let target_name = target
                .file_name()
                .and_then(|n| n.to_str())
                .expect("non-UTF8 artifact directory")
                .to_owned();
            for version in std::fs::read_dir(&target).expect("unreadable artifact directory") {
                let version = version.expect("unreadable version entry").path();
                let version_name = version
                    .file_name()
                    .and_then(|n| n.to_str())
                    .expect("non-UTF8 version directory")
                    .to_owned();
                on_disk.insert((target_name.clone(), version_name));
            }
        }

        let catalogued = ArtifactId::ALL
            .iter()
            .map(|id| id.metadata())
            .flat_map(|artifact| {
                artifact
                    .releases
                    .iter()
                    .map(move |r| (artifact.cargo_target_name.to_owned(), r.version.to_owned()))
            })
            .collect::<std::collections::BTreeSet<_>>();

        let orphaned = on_disk.difference(&catalogued).collect::<Vec<_>>();
        let phantom = catalogued.difference(&on_disk).collect::<Vec<_>>();

        assert!(
            orphaned.is_empty() && phantom.is_empty(),
            "res/near and the catalog disagree.\n\
             On disk but not catalogued (delete, or add a release entry): {orphaned:?}\n\
             Catalogued but not on disk (commit the blob): {phantom:?}",
        );
    }

    /// **Version-key drift check** — verifies that every artifact's
    /// `metadata.version` matches the actual Cargo.toml version of its
    /// package. Requires the `workspace-loader` feature because version
    /// resolution depends on `cargo_metadata`.
    ///
    /// If this test fails, the catalog in `ids.rs` has a stale version for
    /// at least one artifact, and version keys generated by the gateway will
    /// be wrong. Fix by updating the `version` field in the catalog entry.
    #[test]
    #[cfg(feature = "workspace-loader")]
    #[ignore = "requires cargo metadata and workspace access"]
    fn embedded_version_drift_check() {
        use crate::workspace_loader::{find_package, get_metadata};

        let workspace_dir = std::path::Path::new(env!("CARGO_WORKSPACE_DIR"));
        let metadata = get_metadata(workspace_dir).unwrap_or_else(|e| {
            panic!("Failed to read cargo metadata: {e}");
        });

        for artifact in ArtifactId::ALL.iter().map(|id| id.metadata()) {
            let package = find_package(&metadata, artifact.package_name).unwrap_or_else(|| {
                panic!(
                    "Package '{}' not found in workspace metadata — \
                     catalog may be stale.",
                    artifact.package_name,
                )
            });

            let actual_version = package.version.to_string();
            assert_eq!(
                artifact.version(),
                actual_version,
                "Version drift for {} — the newest catalogued release is '{}' but \
                 Cargo.toml says '{}'.\n\
                 If you bumped the crate version, that release's blob has not been \
                 cut yet: run `just artifact-release {}` to build it reproducibly \
                 and add the release entry. Do NOT simply edit the version string — \
                 the bytes and the version must move together.",
                artifact.package_name,
                artifact.version(),
                actual_version,
                artifact.id,
            );
        }
    }
}
