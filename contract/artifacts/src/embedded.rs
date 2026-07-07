//! Compile-time WASM blobs via `include_bytes!`.
//!
//! Enabled via the `embedded-wasm` feature.
//!
//! # Where the bytes live
//!
//! Each catalog artifact has its WASM bytes checked in under
//! `contract/artifacts/res/near/{target_name}/{target_name}.wasm`. These files
//! are the **single source of truth** for the embedded bytes and are treated as
//! versioned, pinned release artifacts — source is free to move ahead of a
//! shipped blob. To deliberately ship new bytes for an artifact:
//! ```bash
//! cargo near build reproducible-wasm --manifest-path <source_path>/Cargo.toml
//! cp target/near/<contract>/<contract>.wasm \
//!    contract/artifacts/res/near/<contract>/<contract>.wasm
//! ```
//! then update that entry's `expected_sha256` (and `version`) in `ids.rs`.
//!
//! # Consistency guarantee
//!
//! The [`embedded_drift_check`] test verifies every checked-in blob hashes to
//! the `expected_sha256` pinned in its catalog entry — a pure, in-memory check
//! with no rebuild. A blob change is therefore a reviewable edit: the binary and
//! its pinned hash must change together, or the check fails.
//!
//! Run the drift check (blob hash and catalog version):
//! ```bash
//! ./script/check-artifact-drift.sh
//! ```

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
        let drifted = ArtifactId::ALL
            .iter()
            .map(|id| id.metadata())
            .filter_map(|artifact| {
                let actual = sha256_hex(artifact.id.embedded_bytes());
                (actual != artifact.expected_sha256).then(|| {
                    format!(
                        "  {} — embedded blob is {actual}, catalog pins {}",
                        artifact.package_name, artifact.expected_sha256,
                    )
                })
            })
            .collect::<Vec<_>>();

        assert!(
            drifted.is_empty(),
            "Blob hash drift for {} artifact(s) — embedded bytes do not match the \
             `expected_sha256` pinned in ids.rs:\n{}\n\
             If this is an intended blob change, update each entry's \
             `expected_sha256` (and `version`) to match the new bytes.",
            drifted.len(),
            drifted.join("\n"),
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
                artifact.version, actual_version,
                "Version drift for {} ({}) — catalog version '{}' does not \
                 match Cargo.toml version '{}'.\n\
                 Update the version field in contract/artifacts/src/ids.rs \
                 for this artifact.",
                artifact.package_name, artifact.package_name, artifact.version, actual_version,
            );
        }
    }
}
