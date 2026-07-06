//! Canonical contract artifact identifiers and metadata.
//!
//! Every contract that can be deployed, tested, or managed through the
//! gateway / manager / test-utils toolchain is listed here. The catalog is
//! the single source of truth for artifact names, paths, and how they map
//! to `target/near` directories.

use std::{fmt, str::FromStr};

use serde::Deserialize as _;
use thiserror::Error;

/// Errors returned when parsing a contract artifact identifier.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ArtifactParseError {
    #[error("Unknown contract artifact: {0}")]
    Unknown(String),
}

/// Unique identifier for each deployable contract artifact in the workspace.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ContractArtifact {
    // -- Production contracts --
    Registry,
    Market,
    Vault,
    UniversalAccount,
    ProxyOracle,
    ProxyGovernance,
    LstOracle,
    RedstoneAdapter,
    PythProAdapter,
    // -- Mock / test contracts --
    MockFt,
    MockMt,
    MockOracle,
    MockRefFinance,
    MockReceiver,
}

impl ContractArtifact {
    /// Return the serde kebab-case artifact name.
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// Return the CLI-friendly artifact name.
    pub fn friendly_name(self) -> &'static str {
        self.as_str()
    }

    /// Return this artifact's catalog metadata.
    pub fn metadata(self) -> Option<&'static ArtifactMetadata> {
        artifact_catalog()
            .iter()
            .find(|metadata| metadata.id == self)
    }

    /// Return this artifact's embedded WASM bytes.
    #[cfg(feature = "embedded-wasm")]
    pub fn embedded_bytes(self) -> Result<&'static [u8], crate::EmbeddedError> {
        crate::read_embedded_by_id(self)
    }
}

impl fmt::Display for ContractArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ContractArtifact {
    type Err = ArtifactParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.to_ascii_lowercase();
        let deserializer =
            serde::de::value::StrDeserializer::<serde::de::value::Error>::new(normalized.as_str());
        Self::deserialize(deserializer).map_err(|_| ArtifactParseError::Unknown(s.to_owned()))
    }
}

/// Canonical metadata for a single contract artifact.
///
/// Every field is a compile-time constant derived from the workspace layout
/// and the prebuild script.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct ArtifactMetadata {
    /// The artifact identifier.
    pub id: ContractArtifact,
    /// Cargo package name as declared in the package's `Cargo.toml`.
    pub package_name: &'static str,
    /// Sanitised name used inside `target/near/` (dashes replaced with underscores).
    pub cargo_target_name: &'static str,
    /// Source path relative to the workspace root (e.g. `contract/market`).
    pub source_path: &'static str,
    /// Artifact version used in version keys (`{package}@{version}#{sha256}`).
    ///
    /// Set to the crate/workspace version at the time the embedded WASM blob was
    /// checked in. Updated alongside the blob in `res/near/`.
    pub version: &'static str,
}

/// Return the full catalog of every known contract artifact.
///
/// Order is stable but callers should not depend on a particular ordering.
pub const fn artifact_catalog() -> &'static [ArtifactMetadata] {
    CATALOG
}

// ---------------------------------------------------------------------------
// Catalog definition
// ---------------------------------------------------------------------------

macro_rules! entry {
    ($id:ident, $pkg:expr, $target:expr, $src:expr, $ver:expr) => {
        ArtifactMetadata {
            id: ContractArtifact::$id,
            package_name: $pkg,
            cargo_target_name: $target,
            source_path: $src,
            version: $ver,
        }
    };
}

const CATALOG: &[ArtifactMetadata] = &[
    // -- Production contracts --
    entry!(
        Registry,
        "templar-registry-contract",
        "templar_registry_contract",
        "contract/registry",
        "1.2.1"
    ),
    entry!(
        Market,
        "templar-market-contract",
        "templar_market_contract",
        "contract/market",
        "1.4.0"
    ),
    entry!(
        Vault,
        "templar-vault-contract",
        "templar_vault_contract",
        "contract/vault/near",
        "1.2.1"
    ),
    entry!(
        UniversalAccount,
        "templar-universal-account-contract",
        "templar_universal_account_contract",
        "contract/universal-account",
        "0.5.0"
    ),
    entry!(
        ProxyOracle,
        "templar-proxy-oracle-near-contract",
        "templar_proxy_oracle_near_contract",
        "contract/proxy-oracle/near/contract",
        "0.2.0"
    ),
    entry!(
        ProxyGovernance,
        "templar-proxy-oracle-near-governance-contract",
        "templar_proxy_oracle_near_governance_contract",
        "contract/proxy-oracle/near/governance-contract",
        "0.1.0"
    ),
    entry!(
        LstOracle,
        "templar-lst-oracle-contract",
        "templar_lst_oracle_contract",
        "contract/proxy-oracle/near/lst-contract",
        "1.2.1"
    ),
    entry!(
        RedstoneAdapter,
        "templar-redstone-adapter-contract",
        "templar_redstone_adapter_contract",
        "contract/redstone-adapter",
        "0.1.0"
    ),
    entry!(
        PythProAdapter,
        "templar-pyth-pro-adapter-contract",
        "templar_pyth_pro_adapter_contract",
        "contract/pyth-pro/contract",
        "0.1.0"
    ),
    // -- Mock / test contracts --
    entry!(MockFt, "mock-ft", "mock_ft", "mock/ft", "0.0.0"),
    entry!(MockMt, "mock-mt", "mock_mt", "mock/mt", "0.0.0"),
    entry!(
        MockOracle,
        "mock-oracle",
        "mock_oracle",
        "mock/oracle",
        "0.0.0"
    ),
    entry!(MockRefFinance, "mock-ref", "mock_ref", "mock/ref", "1.2.1"),
    entry!(
        MockReceiver,
        "mock-receiver",
        "mock_receiver",
        "mock/receiver",
        "1.2.1"
    ),
];
