//! Compile-time WASM blobs via `include_bytes!`.
//!
//! Enabled via the `embedded-wasm` feature.
//!
//! # Where the bytes live
//!
//! Each catalog artifact has its WASM bytes checked in under
//! `contract/artifacts/res/near/{target_name}/{target_name}.wasm`. These
//! files are the **single source of truth** for the embedded bytes. They
//! are updated by running `./script/prebuild-test-contracts.sh` and then
//! copying the fresh output from `target/near/` into `res/near/`.
//!
//! # Staleness guarantee
//!
//! The [`embedded_drift_check`] test (ignored by default) compares every
//! checked-in blob against the corresponding `target/near` file. Because
//! `cargo near build reproducible-wasm` embeds the source commit in NEP-330
//! metadata, the comparison canonicalizes only those self-referential commit
//! hashes before comparing bytes. Any other byte drift still fails. If the
//! test fails, the checked-in bytes are stale and must be refreshed.
//!
//! Run the drift check (both byte and version drift):
//! ```bash
//! cargo test -p templar-contract-artifacts \
//!   --features embedded-wasm,workspace-loader \
//!   drift_check -- --ignored --nocapture
//! ```
//! The CI script `./script/test.sh` runs this automatically after prebuilding
//! contracts.

use crate::ArtifactId;
use thiserror::Error;

/// Errors when reading embedded WASM bytes.
#[derive(Error, Debug)]
pub enum EmbeddedError {
    /// The requested artifact is not in the catalog.
    #[error("Unknown artifact")]
    Unknown,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub(crate) fn embedded_bytes(id: ArtifactId) -> &'static [u8] {
    match id {
        ArtifactId::Registry => include_bytes!(
            "../res/near/templar_registry_contract/templar_registry_contract.wasm"
        ),
        ArtifactId::Market => include_bytes!(
            "../res/near/templar_market_contract/templar_market_contract.wasm"
        ),
        ArtifactId::Vault => include_bytes!(
            "../res/near/templar_vault_contract/templar_vault_contract.wasm"
        ),
        ArtifactId::UniversalAccount => include_bytes!(
            "../res/near/templar_universal_account_contract/templar_universal_account_contract.wasm"
        ),
        ArtifactId::ProxyOracle => include_bytes!(
            "../res/near/templar_proxy_oracle_near_contract/templar_proxy_oracle_near_contract.wasm"
        ),
        ArtifactId::ProxyGovernance => include_bytes!(
            "../res/near/templar_proxy_oracle_near_governance_contract/templar_proxy_oracle_near_governance_contract.wasm"
        ),
        ArtifactId::LstOracle => include_bytes!(
            "../res/near/templar_lst_oracle_contract/templar_lst_oracle_contract.wasm"
        ),
        ArtifactId::RedstoneAdapter => include_bytes!(
            "../res/near/templar_redstone_adapter_contract/templar_redstone_adapter_contract.wasm"
        ),
        ArtifactId::PythProAdapter => include_bytes!(
            "../res/near/templar_pyth_pro_adapter_contract/templar_pyth_pro_adapter_contract.wasm"
        ),
        ArtifactId::MockFt => include_bytes!("../res/near/mock_ft/mock_ft.wasm"),
        ArtifactId::MockMt => include_bytes!("../res/near/mock_mt/mock_mt.wasm"),
        ArtifactId::MockOracle => include_bytes!("../res/near/mock_oracle/mock_oracle.wasm"),
        ArtifactId::MockRefFinance => include_bytes!("../res/near/mock_ref/mock_ref.wasm"),
        ArtifactId::MockReceiver => include_bytes!("../res/near/mock_receiver/mock_receiver.wasm"),
    }
}

/// Return the size in bytes of the embedded WASM for every catalogued artifact.
pub fn embedded_sizes() -> Vec<(&'static str, usize)> {
    crate::artifact_catalog()
        .iter()
        .map(|artifact| (artifact.id.as_str(), artifact.id.embedded_bytes().len()))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{sha256_hex, wasm_drift::canonicalize_nep330_source_refs};

    /// All 14 catalogued artifacts must return non-empty WASM starting with
    /// the magic bytes. This test fails at compile time if a checked-in file
    /// is missing, and at runtime if a blob is corrupt or empty.
    #[test]
    fn test_read_embedded_all_artifacts() {
        for artifact in crate::artifact_catalog() {
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
        let expected = crate::artifact_catalog()
            .iter()
            .map(|artifact| (artifact.id.as_str(), artifact.id.embedded_bytes().len()))
            .collect::<Vec<_>>();

        assert_eq!(sizes, expected);
        for (name, size) in &sizes {
            assert!(*size > 0, "embedded size for {name} is zero");
        }
    }

    /// **Stale-byte drift check** — compares every checked-in embedded blob
    /// against the corresponding `target/near` file on disk.
    ///
    /// This test is `#[ignore]` because it requires all contracts to be
    /// prebuilt in `target/near`. `cargo near build reproducible-wasm` embeds
    /// the source commit in NEP-330 metadata, so this check canonicalizes only
    /// those source-ref commit hashes before comparing bytes. Any other byte
    /// drift means the embedded blobs need to be refreshed.
    ///
    /// To refresh:
    /// 1. Run `./script/prebuild-test-contracts.sh`
    /// 2. Copy fresh output into `res/near/`
    /// 3. Rebuild and re-run this test
    #[test]
    #[ignore = "requires all prebuilt artifacts in target/near"]
    fn embedded_drift_check() {
        let workspace_dir = std::path::Path::new(env!("CARGO_WORKSPACE_DIR"));

        for artifact in crate::artifact_catalog() {
            let embedded = artifact.id.embedded_bytes();

            let disk_path = artifact.target_near_wasm_path(workspace_dir);
            let disk_bytes = std::fs::read(&disk_path).unwrap_or_else(|e| {
                panic!(
                    "Cannot read {} ({}): {e}.\n\
                     Run ./script/prebuild-test-contracts.sh to generate artifacts.",
                    disk_path.display(),
                    artifact.package_name,
                )
            });

            let embedded_hash = sha256_hex(embedded);
            let disk_hash = sha256_hex(&disk_bytes);
            let canonical_embedded = canonicalize_nep330_source_refs(embedded);
            let canonical_disk = canonicalize_nep330_source_refs(&disk_bytes);
            let canonical_embedded_hash = sha256_hex(&canonical_embedded);
            let canonical_disk_hash = sha256_hex(&canonical_disk);

            assert!(
                canonical_embedded == canonical_disk,
                "Drift detected for {} ({}) — checked-in embedded bytes do \
                 not match current `target/near` output after canonicalizing \
                 NEP-330 source commit refs.\n\
                 Embedded SHA-256: {embedded_hash}\n\
                 Disk     SHA-256: {disk_hash}\n\
                 Embedded canonical SHA-256: {canonical_embedded_hash}\n\
                 Disk     canonical SHA-256: {canonical_disk_hash}\n\
                 The checked-in blobs are stale. Re-run:\n\
                   1. ./script/prebuild-test-contracts.sh\n\
                   2. cp target/near/{{contract}}/{{contract}}.wasm \\\n\
                         contract/artifacts/res/near/{{contract}}/{{contract}}.wasm\n\
                 Then rebuild this crate.",
                artifact.package_name,
                artifact.package_name,
            );
        }
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

        for artifact in crate::artifact_catalog() {
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
