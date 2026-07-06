//! Canonical contract artifact IDs, metadata, and byte-loading helpers for
//! Templar Protocol smart contracts.
//!
//! # Features
//!
//! - **default** — Metadata and artifact IDs only; no WASM bytes.
//! - **workspace-loader** — Read WASM bytes at runtime from
//!   `target/near/{name}/{name}.wasm` and provide a `cargo near build` helper.
//! - **embedded-wasm** — Compile-time WASM blobs via `include_bytes!`.
//! - **clap** — Optional parsing helpers for command-line argument handling.
//!
//! # Version keys
//!
//! Version keys follow the format `{package_name}@{version}#{sha256_hex}`
//! defined by `templar-tools-common`. This crate provides formatting and
//! hashing helpers that produce the same output.

#[cfg(feature = "clap")]
mod clap_impl;
#[cfg(feature = "embedded-wasm")]
mod embedded;
mod ids;
#[cfg(feature = "workspace-loader")]
mod workspace_loader;

#[cfg(feature = "clap")]
pub use clap_impl::{friendly_name, friendly_names, metadata_for, parse_artifact_id};
#[cfg(feature = "embedded-wasm")]
pub use embedded::{embedded_sizes, read_embedded, read_embedded_by_id, EmbeddedError};
pub use ids::{artifact_catalog, ArtifactMetadata, ArtifactParseError, ContractArtifact};
#[cfg(feature = "workspace-loader")]
pub use workspace_loader::{
    build_artifact, load_artifact, load_artifact_bytes, BuildContractError, LoadError,
};

use sha2::Digest;
use std::path::Path;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by artifact operations in the default configuration.
#[derive(Error, Debug)]
pub enum ArtifactError {
    /// No artifact with the given package name exists in the catalog.
    #[error("Unknown contract artifact: {0}")]
    UnknownContract(String),

    /// There is no WASM bytes source available — neither `embedded-wasm` nor
    /// `workspace-loader` is enabled.
    #[error(
        "No WASM byte source available. Enable the `embedded-wasm` or \
         `workspace-loader` feature."
    )]
    NoWasmSource,
}

// ---------------------------------------------------------------------------
// Version keys
// ---------------------------------------------------------------------------

/// Format a version key in the canonical `{name}@{version}#{sha256_hex}` form.
///
/// This matches the format produced by `templar-tools-common::build`.
pub fn format_version_key(name: &str, version: &str, wasm_bytes: &[u8]) -> String {
    let hash = sha2::Sha256::digest(wasm_bytes);
    format!("{name}@{version}#{}", hex::encode(hash))
}

/// Compute the SHA-256 hex digest of a WASM blob.
pub fn sha256_hex(wasm_bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(wasm_bytes))
}

// ---------------------------------------------------------------------------
// Target-path helpers (always available — no feature gate)
// ---------------------------------------------------------------------------

/// Return the expected WASM path for an artifact inside `target/near`.
///
/// The convention is `{workspace_dir}/target/near/{cargo_target_name}/{cargo_target_name}.wasm`.
pub fn target_near_wasm_path(
    workspace_dir: &Path,
    metadata: &ArtifactMetadata,
) -> std::path::PathBuf {
    let name = &metadata.cargo_target_name;
    workspace_dir
        .join("target")
        .join("near")
        .join(name)
        .join(format!("{name}.wasm"))
}

/// Return the Cargo-manifest directory for an artifact, relative to the
/// workspace root.
pub fn manifest_path(metadata: &ArtifactMetadata) -> std::path::PathBuf {
    Path::new(metadata.source_path).to_path_buf()
}

// ---------------------------------------------------------------------------
// artifact_catalog helper
// ---------------------------------------------------------------------------

/// Resolve an artifact by its Cargo package name (e.g. `"templar-market-contract"`).
pub fn find_by_package_name(package: &str) -> Option<&'static ArtifactMetadata> {
    artifact_catalog()
        .iter()
        .find(|a| a.package_name == package)
}

/// Resolve an artifact by its [`ContractArtifact`] ID.
///
/// Returns `Ok(metadata)` for every variant in the catalog. A missing ID
/// indicates a program bug (new variant added without a catalog entry), which
/// this function reports as an [`ArtifactError::UnknownContract`].
pub fn find_by_id(id: ContractArtifact) -> Result<&'static ArtifactMetadata, ArtifactError> {
    id.metadata()
        .ok_or_else(|| ArtifactError::UnknownContract(id.to_string()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_version_key() {
        let key = format_version_key("templar-market-contract", "1.2.1", b"hello wasm");
        assert!(key.starts_with("templar-market-contract@1.2.1#"));
        assert_eq!(key.len(), "templar-market-contract@1.2.1#".len() + 64);

        // SHA-256 of "hello wasm" (hardcoded for determinism)
        let expected_hash = sha256_hex(b"hello wasm");
        assert_eq!(
            key,
            format!("templar-market-contract@1.2.1#{expected_hash}")
        );
    }

    #[test]
    fn test_sha256_hex() {
        let digest = sha256_hex(b"hello wasm");
        assert_eq!(digest.len(), 64);
        // Known SHA-256 of "hello wasm"
        assert_eq!(
            digest,
            "136f0dec77ef3c5570737642efa4c7e150d23a492a37fc5b2eff183ef7084f02"
        );
    }

    #[test]
    fn test_target_near_wasm_path() {
        let meta = ArtifactMetadata {
            id: ContractArtifact::Market,
            package_name: "templar-market-contract",
            cargo_target_name: "templar_market_contract",
            source_path: "contract/market",
            version: "1.4.0",
        };
        let path = target_near_wasm_path(Path::new("/ws"), &meta);
        assert_eq!(
            path,
            Path::new("/ws/target/near/templar_market_contract/templar_market_contract.wasm")
        );
    }

    #[test]
    fn test_find_by_package_name() {
        let meta = find_by_package_name("templar-market-contract");
        assert!(meta.is_some());
        assert_eq!(meta.unwrap().id, ContractArtifact::Market);
    }

    #[test]
    fn test_find_by_id() {
        let meta = find_by_id(ContractArtifact::Vault).unwrap();
        assert_eq!(meta.id, ContractArtifact::Vault);
        assert_eq!(meta.package_name, "templar-vault-contract");
        assert_eq!(meta.version, "1.2.1");
    }

    #[test]
    fn test_find_by_id_returns_all_catalog() {
        for meta in artifact_catalog() {
            let found = find_by_id(meta.id).unwrap();
            assert_eq!(found, meta);
        }
    }

    #[test]
    fn test_find_unknown_package() {
        assert!(find_by_package_name("no-such-contract").is_none());
    }

    #[test]
    fn test_catalog_has_all_entries() {
        let catalog = artifact_catalog();
        assert!(!catalog.is_empty());
        // All IDs must be unique
        let mut ids: Vec<_> = catalog.iter().map(|a| &a.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), catalog.len(), "duplicate artifact IDs");

        // All package names must be unique
        let mut packages: Vec<_> = catalog.iter().map(|a| a.package_name).collect();
        packages.sort_unstable();
        packages.dedup();
        assert_eq!(packages.len(), catalog.len(), "duplicate package names");
    }

    #[test]
    fn test_catalog_target_names_match_convention() {
        for artifact in artifact_catalog() {
            let target_name = artifact.cargo_target_name;
            assert!(
                !target_name.contains('-'),
                "target name {target_name} for {} must not contain dashes",
                artifact.package_name,
            );
        }
    }

    #[test]
    fn test_contract_artifact_serde_kebab_case() {
        let json = serde_json::to_string(&ContractArtifact::Market).unwrap();
        assert_eq!(json, r#""market""#);

        let json = serde_json::to_string(&ContractArtifact::UniversalAccount).unwrap();
        assert_eq!(json, r#""universal-account""#);

        let json = serde_json::to_string(&ContractArtifact::MockFt).unwrap();
        assert_eq!(json, r#""mock-ft""#);

        let json = serde_json::to_string(&ContractArtifact::MockRefFinance).unwrap();
        assert_eq!(json, r#""mock-ref-finance""#);

        let id: ContractArtifact = serde_json::from_str(r#""proxy-oracle""#).unwrap();
        assert_eq!(id, ContractArtifact::ProxyOracle);

        let id: ContractArtifact = serde_json::from_str(r#""redstone-adapter""#).unwrap();
        assert_eq!(id, ContractArtifact::RedstoneAdapter);
    }

    #[test]
    fn test_contract_artifact_display_and_parse_use_kebab_case_names() {
        assert_eq!(
            ContractArtifact::UniversalAccount.as_str(),
            "universal-account"
        );
        assert_eq!(
            ContractArtifact::MockRefFinance.to_string(),
            "mock-ref-finance"
        );
        assert_eq!(
            "PYTH-PRO-ADAPTER".parse::<ContractArtifact>().unwrap(),
            ContractArtifact::PythProAdapter,
        );
        assert!("no-such-artifact".parse::<ContractArtifact>().is_err());
    }

    #[test]
    fn test_contract_artifact_metadata_method() {
        let metadata = ContractArtifact::Market.metadata().unwrap();
        assert_eq!(metadata.package_name, "templar-market-contract");
        assert_eq!(metadata.source_path, "contract/market");
    }

    #[test]
    fn test_artifact_metadata_serde() {
        let meta = find_by_package_name("templar-market-contract").unwrap();
        let json = serde_json::to_string(meta).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["id"], "market");
        assert_eq!(parsed["package_name"], "templar-market-contract");
        assert_eq!(parsed["cargo_target_name"], "templar_market_contract");
        assert_eq!(parsed["source_path"], "contract/market");
        assert_eq!(parsed["version"], "1.4.0");
    }
}
