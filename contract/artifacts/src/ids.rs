//! Canonical contract artifact identifiers and metadata.
//!
//! Every contract that can be deployed, tested, or managed through the
//! gateway / manager / test-utils toolchain is listed here. The catalog is
//! the single source of truth for artifact names, paths, and how they map
//! to `target/near` directories.

use std::{fmt, path::Path, str::FromStr};

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
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ArtifactId {
    // -- Production contracts --
    #[cfg_attr(feature = "clap", value(alias = "templar-registry-contract"))]
    Registry,
    #[cfg_attr(feature = "clap", value(alias = "templar-market-contract"))]
    Market,
    #[cfg_attr(feature = "clap", value(alias = "templar-vault-contract"))]
    Vault,
    #[cfg_attr(feature = "clap", value(alias = "templar-universal-account-contract"))]
    UniversalAccount,
    #[cfg_attr(feature = "clap", value(alias = "templar-proxy-oracle-near-contract"))]
    ProxyOracle,
    #[cfg_attr(
        feature = "clap",
        value(alias = "templar-proxy-oracle-near-governance-contract")
    )]
    ProxyGovernance,
    #[cfg_attr(feature = "clap", value(alias = "templar-lst-oracle-contract"))]
    LstOracle,
    #[cfg_attr(feature = "clap", value(alias = "templar-redstone-adapter-contract"))]
    RedstoneAdapter,
    #[cfg_attr(feature = "clap", value(alias = "templar-pyth-pro-adapter-contract"))]
    PythProAdapter,
    // -- Mock / test contracts --
    MockFt,
    MockMt,
    MockOracle,
    #[cfg_attr(feature = "clap", value(alias = "mock-ref"))]
    MockRefFinance,
    MockReceiver,
}

impl ArtifactId {
    pub const ALL: [Self; 14] = [
        Self::Registry,
        Self::Market,
        Self::Vault,
        Self::UniversalAccount,
        Self::ProxyOracle,
        Self::ProxyGovernance,
        Self::LstOracle,
        Self::RedstoneAdapter,
        Self::PythProAdapter,
        Self::MockFt,
        Self::MockMt,
        Self::MockOracle,
        Self::MockRefFinance,
        Self::MockReceiver,
    ];

    pub fn as_str(self) -> &'static str {
        self.into()
    }

    pub fn metadata(self) -> &'static ArtifactMetadata {
        match self {
            Self::Registry => &REGISTRY_METADATA,
            Self::Market => &MARKET_METADATA,
            Self::Vault => &VAULT_METADATA,
            Self::UniversalAccount => &UNIVERSAL_ACCOUNT_METADATA,
            Self::ProxyOracle => &PROXY_ORACLE_METADATA,
            Self::ProxyGovernance => &PROXY_GOVERNANCE_METADATA,
            Self::LstOracle => &LST_ORACLE_METADATA,
            Self::RedstoneAdapter => &REDSTONE_ADAPTER_METADATA,
            Self::PythProAdapter => &PYTH_PRO_ADAPTER_METADATA,
            Self::MockFt => &MOCK_FT_METADATA,
            Self::MockMt => &MOCK_MT_METADATA,
            Self::MockOracle => &MOCK_ORACLE_METADATA,
            Self::MockRefFinance => &MOCK_REF_FINANCE_METADATA,
            Self::MockReceiver => &MOCK_RECEIVER_METADATA,
        }
    }

    pub fn from_package_name(package_name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|id| id.metadata().package_name == package_name)
    }

    #[cfg(feature = "embedded-wasm")]
    pub fn embedded_bytes(self) -> &'static [u8] {
        match self {
            Self::Registry => include_bytes!(
                "../res/near/templar_registry_contract/templar_registry_contract.wasm"
            ),
            Self::Market => include_bytes!(
                "../res/near/templar_market_contract/templar_market_contract.wasm"
            ),
            Self::Vault => include_bytes!(
                "../res/near/templar_vault_contract/templar_vault_contract.wasm"
            ),
            Self::UniversalAccount => include_bytes!(
                "../res/near/templar_universal_account_contract/templar_universal_account_contract.wasm"
            ),
            Self::ProxyOracle => include_bytes!(
                "../res/near/templar_proxy_oracle_near_contract/templar_proxy_oracle_near_contract.wasm"
            ),
            Self::ProxyGovernance => include_bytes!(
                "../res/near/templar_proxy_oracle_near_governance_contract/templar_proxy_oracle_near_governance_contract.wasm"
            ),
            Self::LstOracle => include_bytes!(
                "../res/near/templar_lst_oracle_contract/templar_lst_oracle_contract.wasm"
            ),
            Self::RedstoneAdapter => include_bytes!(
                "../res/near/templar_redstone_adapter_contract/templar_redstone_adapter_contract.wasm"
            ),
            Self::PythProAdapter => include_bytes!(
                "../res/near/templar_pyth_pro_adapter_contract/templar_pyth_pro_adapter_contract.wasm"
            ),
            Self::MockFt => include_bytes!("../res/near/mock_ft/mock_ft.wasm"),
            Self::MockMt => include_bytes!("../res/near/mock_mt/mock_mt.wasm"),
            Self::MockOracle => include_bytes!("../res/near/mock_oracle/mock_oracle.wasm"),
            Self::MockRefFinance => include_bytes!("../res/near/mock_ref/mock_ref.wasm"),
            Self::MockReceiver => include_bytes!("../res/near/mock_receiver/mock_receiver.wasm"),
        }
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ArtifactId {
    type Err = ArtifactParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.to_ascii_lowercase();
        let deserializer =
            serde::de::value::StrDeserializer::<serde::de::value::Error>::new(normalized.as_str());
        if let Ok(id) = Self::deserialize(deserializer) {
            return Ok(id);
        }
        Self::from_package_name(s).ok_or_else(|| ArtifactParseError::Unknown(s.to_owned()))
    }
}

/// Canonical metadata for a single contract artifact.
///
/// Every field is a compile-time constant derived from the workspace layout
/// and the prebuild script.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct ArtifactMetadata {
    /// The artifact identifier.
    pub id: ArtifactId,
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
    /// SHA-256 (lowercase hex) of the checked-in `res/near/` blob.
    ///
    /// Pinned so a blob change is a reviewable, greppable edit rather than an
    /// opaque binary diff: any change to the embedded bytes must land with a
    /// matching update here, or the drift check fails. See `embedded_drift_check`.
    pub expected_sha256: &'static str,
}

impl ArtifactMetadata {
    pub fn manifest_path(&self) -> std::path::PathBuf {
        Path::new(self.source_path).join("Cargo.toml")
    }

    pub fn version_key(&self, wasm_bytes: &[u8]) -> String {
        crate::format_version_key(self.package_name, self.version, wasm_bytes)
    }
}

/// Return the full catalog of every known contract artifact.
///
/// Order is stable but callers should not depend on a particular ordering.
pub fn artifact_catalog() -> impl ExactSizeIterator<Item = &'static ArtifactMetadata> {
    ArtifactId::ALL.iter().map(|id| id.metadata())
}

// ---------------------------------------------------------------------------
// Catalog definition
// ---------------------------------------------------------------------------

macro_rules! entry {
    ($id:ident, $pkg:expr, $target:expr, $src:expr, $ver:expr, $sha:expr) => {
        ArtifactMetadata {
            id: ArtifactId::$id,
            package_name: $pkg,
            cargo_target_name: $target,
            source_path: $src,
            version: $ver,
            expected_sha256: $sha,
        }
    };
}

static REGISTRY_METADATA: ArtifactMetadata = entry!(
    Registry,
    "templar-registry-contract",
    "templar_registry_contract",
    "contract/registry",
    "1.2.1",
    "2512b842e31f8427fb0a47df4f1592de6babf6b13171af60069e3cf450423aa2"
);
static MARKET_METADATA: ArtifactMetadata = entry!(
    Market,
    "templar-market-contract",
    "templar_market_contract",
    "contract/market",
    "1.4.0",
    "8f2c487ebc873e3d6de7e8d2dc4d20b142ab22073e04ae89687746ebaca6ca52"
);
static VAULT_METADATA: ArtifactMetadata = entry!(
    Vault,
    "templar-vault-contract",
    "templar_vault_contract",
    "contract/vault/near",
    "1.2.1",
    "fc605cd4a3e09fdef3620ed9c6a4610bb639d7a5f625d648780a96b7d452ef18"
);
static UNIVERSAL_ACCOUNT_METADATA: ArtifactMetadata = entry!(
    UniversalAccount,
    "templar-universal-account-contract",
    "templar_universal_account_contract",
    "contract/universal-account",
    "0.5.0",
    "7dae78aaf868844af5655d530c50f72a4a74baed92d1592c89c704989e4589c7"
);
static PROXY_ORACLE_METADATA: ArtifactMetadata = entry!(
    ProxyOracle,
    "templar-proxy-oracle-near-contract",
    "templar_proxy_oracle_near_contract",
    "contract/proxy-oracle/near/contract",
    "0.2.0",
    "579e3bba60aa09a4f5f5fbe5d92a6436e7fac45e8c6a9aebdf90c22d8d9d220a"
);
static PROXY_GOVERNANCE_METADATA: ArtifactMetadata = entry!(
    ProxyGovernance,
    "templar-proxy-oracle-near-governance-contract",
    "templar_proxy_oracle_near_governance_contract",
    "contract/proxy-oracle/near/governance-contract",
    "0.1.0",
    "084d6107838c7bd4d250237dafa12a618053dabadf20b70141db02fc501f7bdd"
);
static LST_ORACLE_METADATA: ArtifactMetadata = entry!(
    LstOracle,
    "templar-lst-oracle-contract",
    "templar_lst_oracle_contract",
    "contract/proxy-oracle/near/lst-contract",
    "1.2.1",
    "5ef7bedf78f3a3ecc9747b8aa3bb25ef6bd508d3c17ec166571f76f53ce8ee50"
);
static REDSTONE_ADAPTER_METADATA: ArtifactMetadata = entry!(
    RedstoneAdapter,
    "templar-redstone-adapter-contract",
    "templar_redstone_adapter_contract",
    "contract/redstone-adapter",
    "0.1.0",
    "c31323328f575cef844fe6c1aaa549a4b164b17147d58e30ee0435f799b36658"
);
static PYTH_PRO_ADAPTER_METADATA: ArtifactMetadata = entry!(
    PythProAdapter,
    "templar-pyth-pro-adapter-contract",
    "templar_pyth_pro_adapter_contract",
    "contract/pyth-pro/contract",
    "0.1.0",
    "3b12f98982406ef9333370104dd496b695783d4fed07cdf387f3100e4102f703"
);
static MOCK_FT_METADATA: ArtifactMetadata = entry!(
    MockFt,
    "mock-ft",
    "mock_ft",
    "mock/ft",
    "0.0.0",
    "c43561acd98e1a8d93ba85955f23847bcccd738f85703e914c3d4218471c262d"
);
static MOCK_MT_METADATA: ArtifactMetadata = entry!(
    MockMt,
    "mock-mt",
    "mock_mt",
    "mock/mt",
    "0.0.0",
    "cd125c142722e48e5e1b6f350c751ed0691221d065c540dad02804dbaf55b453"
);
static MOCK_ORACLE_METADATA: ArtifactMetadata = entry!(
    MockOracle,
    "mock-oracle",
    "mock_oracle",
    "mock/oracle",
    "0.0.0",
    "e03c7a051fbdab9eb0fc0e7d4426ebfe4cc9732e500ca575ccf0c860938dfbb9"
);
static MOCK_REF_FINANCE_METADATA: ArtifactMetadata = entry!(
    MockRefFinance,
    "mock-ref",
    "mock_ref",
    "mock/ref",
    "1.2.1",
    "d2be82aa462f55baa2333bae898bee424560e7bf7342b606f6bd40c1aa8369c4"
);
static MOCK_RECEIVER_METADATA: ArtifactMetadata = entry!(
    MockReceiver,
    "mock-receiver",
    "mock_receiver",
    "mock/receiver",
    "1.2.1",
    "bf814164155b927b4e703d8e870137b7f8ab39cd2a69666a0b900bfe0c8a8ec5"
);
