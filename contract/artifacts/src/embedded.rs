//! Compile-time WASM blobs via `include_bytes!`.
//!
//! Enabled via the `embedded-wasm` feature.
//!
//! # Where the bytes live
//!
//! Each catalog artifact has its WASM bytes checked in under
//! `contract/artifacts/res/near/{target_name}/{target_name}.wasm`. These
//! files are the **single source of truth** for the embedded bytes. They
//! are updated by running `./script/prebuild-test-contracts.sh --profile drift`
//! and then copying the fresh output from Cargo's resolved `target/near/`
//! directory into `res/near/`.
//!
//! # Staleness guarantee
//!
//! The [`embedded_drift_check`] test (ignored by default) compares every
//! checked-in blob against the corresponding file under Cargo's resolved
//! `target/near/` directory. Because
//! `cargo near build reproducible-wasm` embeds the source commit in NEP-330
//! metadata, the comparison canonicalizes only those self-referential commit
//! hashes before comparing bytes. Any other byte drift still fails. If the
//! test fails, the checked-in bytes are stale and must be refreshed.
//!
//! Run the drift check (both byte and version drift):
//! ```bash
//! ./script/check-artifact-drift.sh
//! ```
//! CI runs this separately from ordinary integration tests.

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

    /// **Stale-byte drift check** — compares every checked-in embedded blob
    /// against the corresponding file under Cargo's resolved `target/near`
    /// directory on disk.
    ///
    /// This test is `#[ignore]` because it requires all contracts to be
    /// prebuilt in Cargo's resolved `target/near` directory. `cargo near build reproducible-wasm` embeds
    /// the source commit in NEP-330 metadata, so this check canonicalizes only
    /// those source-ref commit hashes before comparing bytes. Any other byte
    /// drift means the embedded blobs need to be refreshed.
    ///
    /// To refresh:
    /// 1. Run `./script/prebuild-test-contracts.sh --profile drift`
    /// 2. Copy fresh output into `res/near/`
    /// 3. Rebuild and re-run this test
    #[test]
    #[cfg(feature = "workspace-loader")]
    #[ignore = "requires all prebuilt artifacts in target/near"]
    fn embedded_drift_check() {
        use crate::{
            sha256_hex,
            wasm_drift::canonicalize_nep330_source_refs,
            workspace_loader::{get_metadata, target_near_wasm_path_from_meta},
        };

        let workspace_dir = std::path::Path::new(env!("CARGO_WORKSPACE_DIR"));
        let metadata = get_metadata(workspace_dir).unwrap_or_else(|e| {
            panic!("Failed to read cargo metadata: {e}");
        });

        // Collect every drifted artifact rather than panicking on the first, so
        // one run of this test reports all stale blobs. Otherwise a batch merge
        // that staled several blobs forces a fix-one, rebuild (~25min), repeat
        // loop, since each panic hides the artifacts after it.
        let mut drifted = Vec::new();

        for artifact in ArtifactId::ALL.iter().map(|id| id.metadata()) {
            let embedded = artifact.id.embedded_bytes();

            let disk_path = target_near_wasm_path_from_meta(
                metadata.target_directory.as_std_path(),
                artifact.cargo_target_name,
            );
            let disk_bytes = std::fs::read(&disk_path).unwrap_or_else(|e| {
                panic!(
                    "Cannot read {} ({}): {e}.\n\
                     Run ./script/prebuild-test-contracts.sh --profile drift to generate artifacts.",
                    disk_path.display(),
                    artifact.package_name,
                )
            });

            let canonical_embedded = canonicalize_nep330_source_refs(embedded);
            let canonical_disk = canonicalize_nep330_source_refs(&disk_bytes);
            if canonical_embedded == canonical_disk {
                continue;
            }

            drifted.push(format!(
                "  {} — embedded SHA {} (canonical {}) vs disk SHA {} (canonical {})",
                artifact.package_name,
                sha256_hex(embedded),
                sha256_hex(&canonical_embedded),
                sha256_hex(&disk_bytes),
                sha256_hex(&canonical_disk),
            ));
        }

        assert!(
            drifted.is_empty(),
            "Drift detected for {} artifact(s) — checked-in embedded bytes do not \
             match current `target/near` output after canonicalizing NEP-330 source \
             commit refs:\n{}\n\
             The checked-in blobs are stale. Re-run:\n\
               1. ./script/prebuild-test-contracts.sh --profile drift\n\
               2. cp target/near/{{contract}}/{{contract}}.wasm \\\n\
                     contract/artifacts/res/near/{{contract}}/{{contract}}.wasm\n\
             for each artifact above, then rebuild this crate.",
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
