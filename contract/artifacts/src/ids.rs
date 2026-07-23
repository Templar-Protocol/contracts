//! Canonical contract artifact identifiers and metadata.
//!
//! Every contract that can be deployed, tested, or managed through the
//! gateway / manager / test-utils toolchain is listed here. The catalog is
//! the single source of truth for artifact names, paths, and how they map
//! to `target/near` directories.
//!
//! ⚠️ The `expected_sha256` / `version` in each entry pin a *release* blob under
//! `res/near/`, NOT a mirror of current source. Changing a contract's source
//! does NOT refresh its blob, and CI will NOT catch a stale blob (the hash-pin
//! check compares blob vs pin, never blob vs source). When you want a source
//! change to become what the gateway deploys, follow the refresh procedure in
//! `contract/artifacts/README.md` ("Refreshing a checked-in blob") and update
//! the blob + `expected_sha256` (+ `version`) together. Bumping a contract's
//! `Cargo.toml` version fails the version-drift check until this catalog's
//! `version` is updated — treat that as your cue to do the full refresh.

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
    #[cfg_attr(feature = "clap", value(alias = "templar-pyth-lazer-adapter-contract"))]
    PythLazerAdapter,
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
        Self::PythLazerAdapter,
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
            Self::PythLazerAdapter => &PYTH_LAZER_ADAPTER_METADATA,
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

    /// Bytes of the **newest** released version — what the gateway deploys.
    #[cfg(feature = "embedded-wasm")]
    // Infallible: `current()` names a catalogued release, and
    // `catalog_releases_have_embedded_bytes` fails if any release lacks an arm.
    #[allow(clippy::expect_used)]
    pub fn embedded_bytes(self) -> &'static [u8] {
        self.embedded_bytes_for_version(self.metadata().version())
            .expect("catalog `current` release has no embedded blob")
    }

    /// Bytes of a specific released version, or `None` if that version was never
    /// released. Used by migration and upgrade tests to deploy the real
    /// historical binary rather than an approximation of it.
    #[cfg(feature = "embedded-wasm")]
    // An exhaustive (artifact, version) → blob table, one arm per catalogued
    // release. It grows by one arm per release and has no branching logic to
    // factor out; splitting it would only scatter the mapping.
    #[allow(clippy::too_many_lines)]
    pub fn embedded_bytes_for_version(self, version: &str) -> Option<&'static [u8]> {
        // Every arm mirrors one `ArtifactRelease` in the catalog below;
        // `catalog_releases_have_embedded_bytes` fails if the two drift apart.
        let bytes: &'static [u8] = match (self, version) {
            (Self::Registry, "1.2.1") => include_bytes!(concat!(
                "../res/near/",
                "templar_registry_contract",
                "/",
                "1.2.1",
                "/",
                "templar_registry_contract",
                ".wasm"
            )),
            (Self::Market, "1.4.0") => include_bytes!(concat!(
                "../res/near/",
                "templar_market_contract",
                "/",
                "1.4.0",
                "/",
                "templar_market_contract",
                ".wasm"
            )),
            (Self::Vault, "1.2.1") => include_bytes!(concat!(
                "../res/near/",
                "templar_vault_contract",
                "/",
                "1.2.1",
                "/",
                "templar_vault_contract",
                ".wasm"
            )),
            (Self::UniversalAccount, "0.2.0") => include_bytes!(concat!(
                "../res/near/",
                "templar_universal_account_contract",
                "/",
                "0.2.0",
                "/",
                "templar_universal_account_contract",
                ".wasm"
            )),
            (Self::UniversalAccount, "0.4.0") => include_bytes!(concat!(
                "../res/near/",
                "templar_universal_account_contract",
                "/",
                "0.4.0",
                "/",
                "templar_universal_account_contract",
                ".wasm"
            )),
            (Self::UniversalAccount, "0.5.0") => include_bytes!(concat!(
                "../res/near/",
                "templar_universal_account_contract",
                "/",
                "0.5.0",
                "/",
                "templar_universal_account_contract",
                ".wasm"
            )),
            (Self::ProxyOracle, "0.1.0") => include_bytes!(concat!(
                "../res/near/",
                "templar_proxy_oracle_near_contract",
                "/",
                "0.1.0",
                "/",
                "templar_proxy_oracle_near_contract",
                ".wasm"
            )),
            (Self::ProxyOracle, "0.3.0") => include_bytes!(concat!(
                "../res/near/",
                "templar_proxy_oracle_near_contract",
                "/",
                "0.3.0",
                "/",
                "templar_proxy_oracle_near_contract",
                ".wasm"
            )),
            (Self::ProxyGovernance, "0.1.0") => include_bytes!(concat!(
                "../res/near/",
                "templar_proxy_oracle_near_governance_contract",
                "/",
                "0.1.0",
                "/",
                "templar_proxy_oracle_near_governance_contract",
                ".wasm"
            )),
            (Self::LstOracle, "1.2.1") => include_bytes!(concat!(
                "../res/near/",
                "templar_lst_oracle_contract",
                "/",
                "1.2.1",
                "/",
                "templar_lst_oracle_contract",
                ".wasm"
            )),
            (Self::RedstoneAdapter, "0.2.0") => include_bytes!(concat!(
                "../res/near/",
                "templar_redstone_adapter_contract",
                "/",
                "0.2.0",
                "/",
                "templar_redstone_adapter_contract",
                ".wasm"
            )),
            (Self::PythLazerAdapter, "0.1.0") => include_bytes!(concat!(
                "../res/near/",
                "templar_pyth_lazer_adapter_contract",
                "/",
                "0.1.0",
                "/",
                "templar_pyth_lazer_adapter_contract",
                ".wasm"
            )),
            (Self::MockFt, "0.0.0") => include_bytes!(concat!(
                "../res/near/",
                "mock_ft",
                "/",
                "0.0.0",
                "/",
                "mock_ft",
                ".wasm"
            )),
            (Self::MockMt, "0.0.0") => include_bytes!(concat!(
                "../res/near/",
                "mock_mt",
                "/",
                "0.0.0",
                "/",
                "mock_mt",
                ".wasm"
            )),
            (Self::MockOracle, "0.0.0") => include_bytes!(concat!(
                "../res/near/",
                "mock_oracle",
                "/",
                "0.0.0",
                "/",
                "mock_oracle",
                ".wasm"
            )),
            (Self::MockRefFinance, "1.2.1") => include_bytes!(concat!(
                "../res/near/",
                "mock_ref",
                "/",
                "1.2.1",
                "/",
                "mock_ref",
                ".wasm"
            )),
            (Self::MockReceiver, "1.2.1") => include_bytes!(concat!(
                "../res/near/",
                "mock_receiver",
                "/",
                "1.2.1",
                "/",
                "mock_receiver",
                ".wasm"
            )),
            _ => return None,
        };
        Some(bytes)
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

/// One released version of a contract, and the SHA-256 of the exact bytes that
/// were shipped for it.
///
/// Releases are immutable. Cutting a new release *adds* an entry and a directory
/// under `res/near/{cargo_target_name}/{version}/`; it never rewrites an
/// existing one. That is what lets migration and upgrade tests deploy the real
/// historical bytes rather than an approximation of them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct ArtifactRelease {
    /// Crate version this blob was released as.
    pub version: &'static str,
    /// SHA-256 (lowercase hex) of the checked-in blob for this version.
    ///
    /// Pinned so a blob change is a reviewable, greppable edit rather than an
    /// opaque binary diff: any change to the bytes must land with a matching
    /// update here, or the drift check fails.
    pub sha256: &'static str,
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
    /// Every released version of this contract, **oldest first**.
    ///
    /// Never empty: an artifact with no release has no bytes to deploy.
    pub releases: &'static [ArtifactRelease],
}

impl ArtifactMetadata {
    pub fn manifest_path(&self) -> std::path::PathBuf {
        Path::new(self.source_path).join("Cargo.toml")
    }

    /// The newest release — what the gateway deploys and what `version_key`
    /// and `embedded_bytes` refer to by default.
    ///
    /// # Panics
    /// Never in practice: `releases` is non-empty for every catalog entry, and
    /// `catalog_releases_are_well_formed` enforces it.
    // Infallible: every catalog entry ships at least one release, enforced by
    // `catalog_releases_are_well_formed`.
    #[allow(clippy::expect_used)]
    pub fn current(&self) -> &'static ArtifactRelease {
        self.releases
            .last()
            .expect("catalog entry has no releases; see catalog_releases_are_well_formed")
    }

    /// Version of the newest release.
    pub fn version(&self) -> &'static str {
        self.current().version
    }

    /// Pinned SHA-256 of the newest release's blob.
    pub fn expected_sha256(&self) -> &'static str {
        self.current().sha256
    }

    /// Look up a specific released version.
    pub fn release(&self, version: &str) -> Option<&'static ArtifactRelease> {
        self.releases.iter().find(|r| r.version == version)
    }

    /// Workspace-relative directory holding the blob for `version`.
    pub fn release_dir(&self, version: &str) -> std::path::PathBuf {
        Path::new("contract/artifacts/res/near")
            .join(self.cargo_target_name)
            .join(version)
    }

    pub fn version_key(&self, wasm_bytes: &[u8]) -> String {
        crate::format_version_key(self.package_name, self.version(), wasm_bytes)
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
    ($id:ident, $pkg:expr, $target:expr, $src:expr, [$(($ver:expr, $sha:expr)),+ $(,)?]) => {
        ArtifactMetadata {
            id: ArtifactId::$id,
            package_name: $pkg,
            cargo_target_name: $target,
            source_path: $src,
            releases: &[$(ArtifactRelease { version: $ver, sha256: $sha }),+],
        }
    };
}

static REGISTRY_METADATA: ArtifactMetadata = entry!(
    Registry,
    "templar-registry-contract",
    "templar_registry_contract",
    "contract/registry",
    [(
        "1.2.1",
        "2512b842e31f8427fb0a47df4f1592de6babf6b13171af60069e3cf450423aa2"
    ),]
);
static MARKET_METADATA: ArtifactMetadata = entry!(
    Market,
    "templar-market-contract",
    "templar_market_contract",
    "contract/market",
    [(
        "1.4.0",
        "8f2c487ebc873e3d6de7e8d2dc4d20b142ab22073e04ae89687746ebaca6ca52"
    ),]
);
static VAULT_METADATA: ArtifactMetadata = entry!(
    Vault,
    "templar-vault-contract",
    "templar_vault_contract",
    "contract/vault/near",
    [(
        "1.2.1",
        "fc605cd4a3e09fdef3620ed9c6a4610bb639d7a5f625d648780a96b7d452ef18"
    ),]
);
static UNIVERSAL_ACCOUNT_METADATA: ArtifactMetadata = entry!(
    UniversalAccount,
    "templar-universal-account-contract",
    "templar_universal_account_contract",
    "contract/universal-account",
    [
        (
            "0.2.0",
            "25ae83a0ee7d31542bd7b6039549f200cdd96f7bcaef56dd6763dd143ef00c2d"
        ),
        (
            "0.4.0",
            "007d0a4643f63b3b2f543b0033f059ebc38b07365ff86aff1aa4476f6d73f9ae"
        ),
        (
            "0.5.0",
            "7dae78aaf868844af5655d530c50f72a4a74baed92d1592c89c704989e4589c7"
        ),
    ]
);
static PROXY_ORACLE_METADATA: ArtifactMetadata = entry!(
    ProxyOracle,
    "templar-proxy-oracle-near-contract",
    "templar_proxy_oracle_near_contract",
    "contract/proxy-oracle/near/contract",
    [
        (
            "0.1.0",
            "fb697b18f30cc19d4fc43768eae04ae94967663c4744ab052c7119c6d869d53b"
        ),
        (
            "0.3.0",
            "d2e62c4566c98e55121a5aad32e0e5b8cfb911f82aca71dbaeaa83794fed9e8e"
        ),
    ]
);
static PROXY_GOVERNANCE_METADATA: ArtifactMetadata = entry!(
    ProxyGovernance,
    "templar-proxy-oracle-near-governance-contract",
    "templar_proxy_oracle_near_governance_contract",
    "contract/proxy-oracle/near/governance-contract",
    [(
        "0.1.0",
        "09ecfafa86bfdca5e05b9174590cd056d59bf3a9d8727e9d452cfb98701334b0"
    ),]
);
static LST_ORACLE_METADATA: ArtifactMetadata = entry!(
    LstOracle,
    "templar-lst-oracle-contract",
    "templar_lst_oracle_contract",
    "contract/proxy-oracle/near/lst-contract",
    [(
        "1.2.1",
        "5ef7bedf78f3a3ecc9747b8aa3bb25ef6bd508d3c17ec166571f76f53ce8ee50"
    ),]
);
static REDSTONE_ADAPTER_METADATA: ArtifactMetadata = entry!(
    RedstoneAdapter,
    "templar-redstone-adapter-contract",
    "templar_redstone_adapter_contract",
    "contract/redstone-adapter",
    [(
        "0.2.0",
        "b513b2e839ce1ea59ef4c57519ef9482b133f47c81db0d3a54517b2cd251511a"
    ),]
);
static PYTH_LAZER_ADAPTER_METADATA: ArtifactMetadata = entry!(
    PythLazerAdapter,
    "templar-pyth-lazer-adapter-contract",
    "templar_pyth_lazer_adapter_contract",
    "contract/pyth-lazer/contract",
    [(
        "0.1.0",
        "c993256a8b42313b2b0b024c783b4eb5a7be1c8b9f792789cb4f207f7007060b"
    ),]
);
static MOCK_FT_METADATA: ArtifactMetadata = entry!(
    MockFt,
    "mock-ft",
    "mock_ft",
    "mock/ft",
    [(
        "0.0.0",
        "c43561acd98e1a8d93ba85955f23847bcccd738f85703e914c3d4218471c262d"
    ),]
);
static MOCK_MT_METADATA: ArtifactMetadata = entry!(
    MockMt,
    "mock-mt",
    "mock_mt",
    "mock/mt",
    [(
        "0.0.0",
        "cd125c142722e48e5e1b6f350c751ed0691221d065c540dad02804dbaf55b453"
    ),]
);
static MOCK_ORACLE_METADATA: ArtifactMetadata = entry!(
    MockOracle,
    "mock-oracle",
    "mock_oracle",
    "mock/oracle",
    [(
        "0.0.0",
        "76f76816cf4d0ccaf4b4e181ee1104ec8e6cbd13084f311b87a86087099a749c"
    ),]
);
static MOCK_REF_FINANCE_METADATA: ArtifactMetadata = entry!(
    MockRefFinance,
    "mock-ref",
    "mock_ref",
    "mock/ref",
    [(
        "1.2.1",
        "d2be82aa462f55baa2333bae898bee424560e7bf7342b606f6bd40c1aa8369c4"
    ),]
);
static MOCK_RECEIVER_METADATA: ArtifactMetadata = entry!(
    MockReceiver,
    "mock-receiver",
    "mock_receiver",
    "mock/receiver",
    [(
        "1.2.1",
        "bf814164155b927b4e703d8e870137b7f8ab39cd2a69666a0b900bfe0c8a8ec5"
    ),]
);
