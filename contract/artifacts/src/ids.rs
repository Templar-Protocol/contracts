//! Canonical contract artifact identifiers and metadata.
//!
//! Every contract that can be deployed, tested, or managed through the
//! gateway / manager / test-utils toolchain is listed here. The catalog is
//! the single source of truth for artifact names, paths, and how they map
//! to `target/near` directories.
//!
//! ⚠️ [`ArtifactMetadata::releases`] is the *released* history of a contract,
//! compiled in from `releases/` — NOT a mirror of current source, which is
//! expected to run ahead of the newest release.
//!
//! Releases are immutable: ship new bytes by *adding* one, never by editing an
//! entry, because historical releases are what the migration and upgrade tests
//! deploy. Cutting one is not a manual step — see `RELEASING.md`.

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

/// One released version of a contract.
///
/// The canonical build for a version that was *released*, not merely bumped.
/// Appended by CI on a release tag, never by hand.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct ArtifactRelease {
    /// Crate version this was released as.
    pub version: &'static str,
    /// Git tag carrying the release, exactly as it exists.
    ///
    /// Recorded, not derived: three tag schemes have been used here (`v1.2.1`,
    /// `uac-v0.2.0`, `templar-market-contract-v1.3.0`), so changing
    /// release-plz's `git_tag_name` cannot strand old releases.
    pub tag: &'static str,
    /// Filename of the WASM asset on that release, likewise as it exists.
    pub asset: &'static str,
    /// SHA-256 (lowercase hex) of the released bytes.
    ///
    /// Root of trust: [`crate::fetch`] refuses bytes that do not match this
    /// reviewed, in-repo value.
    pub sha256: &'static str,
    /// Byte length of the released asset.
    ///
    /// Recorded so that sizing a release — what a deploy deposit has to cover —
    /// needs no download.
    pub length: usize,
}

/// Canonical metadata for a single contract artifact.
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
}

impl ArtifactMetadata {
    pub fn manifest_path(&self) -> std::path::PathBuf {
        Path::new(self.source_path).join("Cargo.toml")
    }

    /// Every released version of this contract, **oldest first**, from
    /// `releases/`. Empty for mocks: test scaffolding is never deployed, so
    /// "the canonical bytes of mock-ft" is not a thing that exists.
    pub fn releases(&self) -> &'static [ArtifactRelease] {
        releases_for(self.id)
    }

    /// The newest release — what the gateway deploys. `None` for an artifact
    /// that has never been released (mocks).
    pub fn current(&self) -> Option<&'static ArtifactRelease> {
        self.releases().last()
    }

    pub fn version(&self) -> Option<&'static str> {
        self.current().map(|release| release.version)
    }

    pub fn release(&self, version: &str) -> Option<&'static ArtifactRelease> {
        self.releases().iter().find(|r| r.version == version)
    }
}

/// The catalogued release whose bytes hash to `sha256`, across every artifact.
///
/// Lets a caller holding only a digest reach [`crate::fetch::released_bytes`]. A registry reports
/// the hash it computed from the code itself, so it identifies a release even when the version key
/// does not — several live keys carry no digest at all, and the `{name}@{version}#{sha}` convention
/// is not enforced on-chain.
///
/// Raw digest rather than text because the two spellings never match and failing to match is
/// indistinguishable from "never released": a registry serves base58, the catalog records hex.
///
/// Re-releasing identical bytes under a new version would make this ambiguous; the oldest wins,
/// and either answer yields the same wasm.
pub fn release_by_sha256(sha256: &[u8; 32]) -> Option<(ArtifactId, &'static ArtifactRelease)> {
    let sha256 = hex::encode(sha256);
    ArtifactId::ALL.into_iter().find_map(|artifact| {
        artifact
            .metadata()
            .releases()
            .iter()
            .find(|release| release.sha256 == sha256)
            .map(|release| (artifact, release))
    })
}

/// Which catalogued artifact a release tag belongs to.
///
/// Only asked of a tag release-plz just created; an existing release's tag is
/// recorded in `releases/`, never reconstructed. So a lenient prefix match
/// suffices — it must identify the package, not round-trip.
///
/// `None` means "not a NEAR artifact" (a Soroban tag), not "failed".
pub fn artifact_from_release_tag(tag: &str) -> Option<ArtifactId> {
    ArtifactId::ALL
        .into_iter()
        .filter(|artifact| {
            tag.strip_prefix(artifact.metadata().package_name)
                .is_some_and(|rest| rest.starts_with(|c: char| !c.is_ascii_alphanumeric()))
        })
        // Longest package name wins, so a name that prefixes another cannot
        // claim its tags.
        .max_by_key(|artifact| artifact.metadata().package_name.len())
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
    ($id:ident, $pkg:expr, $target:expr, $src:expr) => {
        ArtifactMetadata {
            id: ArtifactId::$id,
            package_name: $pkg,
            cargo_target_name: $target,
            source_path: $src,
        }
    };
}

// Release lists come from `releases/`, compiled in by `build.rs`.
include!(concat!(env!("OUT_DIR"), "/releases.rs"));

// `source_path` is where a contract lives *today*; several older releases were
// built from paths that have since moved. A verifier reads the historical path
// from the WASM's own build_info, not from here.

static REGISTRY_METADATA: ArtifactMetadata = entry!(
    Registry,
    "templar-registry-contract",
    "templar_registry_contract",
    "contract/registry"
);
static MARKET_METADATA: ArtifactMetadata = entry!(
    Market,
    "templar-market-contract",
    "templar_market_contract",
    "contract/market"
);

static VAULT_METADATA: ArtifactMetadata = entry!(
    Vault,
    "templar-vault-contract",
    "templar_vault_contract",
    "contract/vault/near"
);

static UNIVERSAL_ACCOUNT_METADATA: ArtifactMetadata = entry!(
    UniversalAccount,
    "templar-universal-account-contract",
    "templar_universal_account_contract",
    "contract/universal-account"
);
static PROXY_ORACLE_METADATA: ArtifactMetadata = entry!(
    ProxyOracle,
    "templar-proxy-oracle-near-contract",
    "templar_proxy_oracle_near_contract",
    "contract/proxy-oracle/near/contract"
);
static PROXY_GOVERNANCE_METADATA: ArtifactMetadata = entry!(
    ProxyGovernance,
    "templar-proxy-oracle-near-governance-contract",
    "templar_proxy_oracle_near_governance_contract",
    "contract/proxy-oracle/near/governance-contract"
);
static LST_ORACLE_METADATA: ArtifactMetadata = entry!(
    LstOracle,
    "templar-lst-oracle-contract",
    "templar_lst_oracle_contract",
    "contract/proxy-oracle/near/lst-contract"
);
static REDSTONE_ADAPTER_METADATA: ArtifactMetadata = entry!(
    RedstoneAdapter,
    "templar-redstone-adapter-contract",
    "templar_redstone_adapter_contract",
    "contract/redstone-adapter"
);
static PYTH_LAZER_ADAPTER_METADATA: ArtifactMetadata = entry!(
    PythLazerAdapter,
    "templar-pyth-lazer-adapter-contract",
    "templar_pyth_lazer_adapter_contract",
    "contract/pyth-lazer/contract"
);

// Mocks: Tier C in `release-plz.toml`. Tests build them from source.
static MOCK_FT_METADATA: ArtifactMetadata = entry!(MockFt, "mock-ft", "mock_ft", "mock/ft");
static MOCK_MT_METADATA: ArtifactMetadata = entry!(MockMt, "mock-mt", "mock_mt", "mock/mt");
static MOCK_ORACLE_METADATA: ArtifactMetadata =
    entry!(MockOracle, "mock-oracle", "mock_oracle", "mock/oracle");
static MOCK_REF_FINANCE_METADATA: ArtifactMetadata =
    entry!(MockRefFinance, "mock-ref", "mock_ref", "mock/ref");
static MOCK_RECEIVER_METADATA: ArtifactMetadata = entry!(
    MockReceiver,
    "mock-receiver",
    "mock_receiver",
    "mock/receiver"
);
