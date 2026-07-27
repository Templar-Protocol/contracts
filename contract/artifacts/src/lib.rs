//! Canonical contract artifact IDs, metadata, and byte-loading helpers for
//! Templar Protocol smart contracts.
//!
//! # Features
//!
//! - **default** — Metadata and artifact IDs only; no WASM bytes.
//! - **workspace-loader** — Read WASM bytes at runtime from
//!   `target/near/{name}/{name}.wasm` and provide a `cargo near build` helper.
//!   This is the *work-in-progress* source: whatever the tree currently builds.
//! - **fetch** — Download *released* WASM from its GitHub Release into a shared
//!   local cache, verified against the catalog's SHA-256 pin. This is the
//!   *canonical* source: the exact bytes a version shipped as.
//! - **clap** — Optional parsing helpers for command-line argument handling.
//!
//! # Version keys
//!
//! Version keys follow the format `{package_name}@{version}#{sha256_hex}`
//! defined by `templar-tools-common`. This crate provides formatting and
//! hashing helpers that produce the same output.

#[cfg(test)]
mod catalog;
#[cfg(feature = "fetch")]
pub mod fetch;
mod ids;
#[cfg(all(feature = "workspace-loader", feature = "clap"))]
pub mod prebuild;
#[cfg(feature = "workspace-loader")]
mod workspace_loader;

pub use ids::{
    artifact_catalog, ArtifactId, ArtifactMetadata, ArtifactParseError, ArtifactRelease,
};
#[cfg(feature = "workspace-loader")]
pub use workspace_loader::{
    build_artifact, load_artifact, load_artifact_bytes, BuildContractError, LoadError,
};

use sha2::Digest;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by artifact operations in the default configuration.
#[derive(Error, Debug)]
pub enum ArtifactError {
    /// There is no WASM bytes source available — neither `fetch` nor
    /// `workspace-loader` is enabled.
    #[error("No WASM byte source available. Enable the `fetch` or `workspace-loader` feature.")]
    NoWasmSource,
}

// ---------------------------------------------------------------------------
// Version keys
// ---------------------------------------------------------------------------

/// Format a version key in the canonical `{name}@{version}#{sha256_hex}` form.
///
/// This matches the format produced by `templar-tools-common::build`.
pub fn format_version_key(name: &str, version: &str, wasm_bytes: &[u8]) -> String {
    format!("{name}@{version}#{}", sha256_hex(wasm_bytes))
}

/// Compute the SHA-256 hex digest of a WASM blob.
pub fn sha256_hex(wasm_bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(wasm_bytes))
}

// ---------------------------------------------------------------------------
// Target-path helpers (always available — no feature gate)
// ---------------------------------------------------------------------------

/// Resolve an artifact by its Cargo package name (e.g. `"templar-market-contract"`).
pub fn find_by_package_name(package: &str) -> Option<&'static ArtifactMetadata> {
    ArtifactId::from_package_name(package).map(ArtifactId::metadata)
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
    fn test_find_by_package_name() {
        let meta = find_by_package_name("templar-market-contract");
        assert!(meta.is_some());
        assert_eq!(meta.unwrap().id, ArtifactId::Market);
    }

    #[test]
    fn test_metadata_by_id() {
        let meta = ArtifactId::Vault.metadata();
        assert_eq!(meta.id, ArtifactId::Vault);
        assert_eq!(meta.package_name, "templar-vault-contract");
        // No NEAR vault has shipped yet, so there is no released version.
        assert_eq!(meta.version(), None);
    }

    #[test]
    fn test_metadata_by_id_returns_all_catalog() {
        for id in ArtifactId::ALL {
            let found = id.metadata();
            assert_eq!(found.id, id);
        }
    }

    #[test]
    fn test_find_unknown_package() {
        assert!(find_by_package_name("no-such-contract").is_none());
    }

    #[test]
    fn test_catalog_has_all_entries() {
        let catalog = ArtifactId::ALL
            .iter()
            .map(|id| id.metadata())
            .collect::<Vec<_>>();
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
    fn test_artifact_id_all_preserves_public_order() {
        assert_eq!(
            ArtifactId::ALL,
            [
                ArtifactId::Registry,
                ArtifactId::Market,
                ArtifactId::Vault,
                ArtifactId::UniversalAccount,
                ArtifactId::ProxyOracle,
                ArtifactId::ProxyGovernance,
                ArtifactId::LstOracle,
                ArtifactId::RedstoneAdapter,
                ArtifactId::PythLazerAdapter,
                ArtifactId::MockFt,
                ArtifactId::MockMt,
                ArtifactId::MockOracle,
                ArtifactId::MockRefFinance,
                ArtifactId::MockReceiver,
            ]
        );
    }

    #[test]
    fn test_artifact_catalog_follows_artifact_id_all_order() {
        let catalog_ids = artifact_catalog()
            .map(|metadata| metadata.id)
            .collect::<Vec<_>>();

        assert_eq!(catalog_ids, ArtifactId::ALL);
    }

    #[test]
    fn test_artifact_id_metadata_is_complete() {
        for id in ArtifactId::ALL {
            assert_eq!(id.metadata().id, id);
        }
    }

    #[test]
    fn test_catalog_target_names_match_convention() {
        for artifact in ArtifactId::ALL.iter().map(|id| id.metadata()) {
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
        let json = serde_json::to_string(&ArtifactId::Market).unwrap();
        assert_eq!(json, r#""market""#);

        let json = serde_json::to_string(&ArtifactId::UniversalAccount).unwrap();
        assert_eq!(json, r#""universal-account""#);

        let json = serde_json::to_string(&ArtifactId::MockFt).unwrap();
        assert_eq!(json, r#""mock-ft""#);

        let json = serde_json::to_string(&ArtifactId::MockRefFinance).unwrap();
        assert_eq!(json, r#""mock-ref-finance""#);

        let id: ArtifactId = serde_json::from_str(r#""proxy-oracle""#).unwrap();
        assert_eq!(id, ArtifactId::ProxyOracle);

        let id: ArtifactId = serde_json::from_str(r#""redstone-adapter""#).unwrap();
        assert_eq!(id, ArtifactId::RedstoneAdapter);
    }

    #[test]
    fn test_contract_artifact_display_and_parse_use_kebab_case_names() {
        assert_eq!(ArtifactId::UniversalAccount.as_str(), "universal-account");
        assert_eq!(ArtifactId::MockRefFinance.to_string(), "mock-ref-finance");
        assert_eq!(
            "PYTH-LAZER-ADAPTER".parse::<ArtifactId>().unwrap(),
            ArtifactId::PythLazerAdapter,
        );
        assert!("no-such-artifact".parse::<ArtifactId>().is_err());
    }

    #[test]
    fn test_artifact_id_metadata_method() {
        let metadata = ArtifactId::Market.metadata();
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
        // Versions live in the release list; `version` is a derived accessor,
        // not a serialized field.
        assert_eq!(parsed["releases"][0]["version"], "1.0.0");
        // Newest *released* version — the crate's Cargo.toml is further ahead.
        assert_eq!(meta.version(), Some("1.3.0"));
    }
}
