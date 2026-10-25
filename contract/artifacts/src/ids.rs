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
    #[cfg_attr(feature = "clap", value(alias = "templar-patch-state-contract"))]
    PatchState,
    // -- Mock / test contracts --
    MockFt,
    MockMt,
    MockOracle,
    #[cfg_attr(feature = "clap", value(alias = "mock-ref"))]
    MockRefFinance,
    MockReceiver,
    MockSigner,
    MockDao,
}

impl ArtifactId {
    pub const ALL: [Self; 17] = [
        Self::Registry,
        Self::Market,
        Self::Vault,
        Self::UniversalAccount,
        Self::ProxyOracle,
        Self::ProxyGovernance,
        Self::LstOracle,
        Self::RedstoneAdapter,
        Self::PythLazerAdapter,
        Self::PatchState,
        Self::MockFt,
        Self::MockMt,
        Self::MockOracle,
        Self::MockRefFinance,
        Self::MockReceiver,
        Self::MockSigner,
        Self::MockDao,
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
            Self::PatchState => &PATCH_STATE_METADATA,
            Self::MockFt => &MOCK_FT_METADATA,
            Self::MockMt => &MOCK_MT_METADATA,
            Self::MockOracle => &MOCK_ORACLE_METADATA,
            Self::MockRefFinance => &MOCK_REF_FINANCE_METADATA,
            Self::MockReceiver => &MOCK_RECEIVER_METADATA,
            Self::MockSigner => &MOCK_SIGNER_METADATA,
            Self::MockDao => &MOCK_DAO_METADATA,
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

/// Whether `tag` is a release-plz tag of `package`: the name, then a
/// separator rather than more of a longer name.
fn tag_names_package(tag: &str, package: &str) -> bool {
    tag.strip_prefix(package)
        .is_some_and(|rest| rest.starts_with(|c: char| !c.is_ascii_alphanumeric()))
}

/// The candidate whose package `tag` names, the longest package name winning so
/// a name that prefixes another cannot claim its tags. One matcher for both
/// catalogs, so their tie-breaking cannot diverge.
fn longest_package_match<T>(
    tag: &str,
    candidates: impl IntoIterator<Item = T>,
    package_name: impl Fn(&T) -> &'static str,
) -> Option<T> {
    candidates
        .into_iter()
        .filter(|candidate| tag_names_package(tag, package_name(candidate)))
        .max_by_key(|candidate| package_name(candidate).len())
}

/// Which catalogued artifact a release tag belongs to.
///
/// Only asked of a tag release-plz just created; an existing release's tag is
/// recorded in `releases/`, never reconstructed. So a lenient prefix match
/// suffices — it must identify the package, not round-trip.
///
/// `None` means "not a NEAR artifact" (a Soroban tag), not "failed". For a
/// Soroban tag see [`ReleaseArtifact::from_release_tag`].
pub fn artifact_from_release_tag(tag: &str) -> Option<ArtifactId> {
    longest_package_match(tag, ArtifactId::ALL, |artifact| {
        artifact.metadata().package_name
    })
}

/// A contract of the Soroban proxy-oracle stack that is released as a WASM.
///
/// Not an [`ArtifactId`]: that enum is the gateway's wire type and what every
/// NEAR consumer (`workspace-loader`, `prebuild-test-contracts`, the gateway's
/// artifact listing) iterates, none of which can build or deploy these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SorobanArtifactId {
    ProxyOracle,
    ProxyGovernance,
    Sep40Adapter,
    LazerSource,
    Batcher,
}

impl SorobanArtifactId {
    pub const ALL: [Self; 5] = [
        Self::ProxyOracle,
        Self::ProxyGovernance,
        Self::Sep40Adapter,
        Self::LazerSource,
        Self::Batcher,
    ];

    /// The catalog key: the first column of a `releases/` row and the prefix of
    /// its file name. `soroban-`-prefixed so it cannot meet a NEAR key.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProxyOracle => "soroban-proxy-oracle",
            Self::ProxyGovernance => "soroban-proxy-governance",
            Self::Sep40Adapter => "soroban-sep40-adapter",
            Self::LazerSource => "soroban-lazer-source",
            Self::Batcher => "soroban-batcher",
        }
    }

    pub fn metadata(self) -> &'static SorobanArtifactMetadata {
        match self {
            Self::ProxyOracle => &SOROBAN_PROXY_ORACLE_METADATA,
            Self::ProxyGovernance => &SOROBAN_PROXY_GOVERNANCE_METADATA,
            Self::Sep40Adapter => &SOROBAN_SEP40_ADAPTER_METADATA,
            Self::LazerSource => &SOROBAN_LAZER_SOURCE_METADATA,
            Self::Batcher => &SOROBAN_BATCHER_METADATA,
        }
    }

    fn from_release_tag(tag: &str) -> Option<Self> {
        longest_package_match(tag, Self::ALL, |artifact| artifact.metadata().package_name)
    }
}

impl fmt::Display for SorobanArtifactId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for SorobanArtifactId {
    type Err = ArtifactParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let key = s.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|id| id.as_str() == key || id.metadata().package_name == s)
            .ok_or_else(|| ArtifactParseError::Unknown(s.to_owned()))
    }
}

/// Canonical metadata for one Soroban proxy-oracle contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SorobanArtifactMetadata {
    pub id: SorobanArtifactId,
    /// Cargo package name as declared in the package's `Cargo.toml`.
    pub package_name: &'static str,
    /// The package name with dashes replaced by underscores: the stem of the
    /// WASM `stellar contract build` emits, and of the release asset.
    pub cargo_target_name: &'static str,
    /// Source path relative to the workspace root.
    pub source_path: &'static str,
}

impl SorobanArtifactMetadata {
    /// Every released version, **oldest first**, from `releases/`.
    pub fn releases(&self) -> &'static [ArtifactRelease] {
        soroban_releases_for(self.id)
    }
}

/// An artifact a release can be cut for, whichever toolchain builds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReleaseArtifact {
    Near(ArtifactId),
    Soroban(SorobanArtifactId),
}

impl ReleaseArtifact {
    /// Which catalogued artifact a release-plz tag belongs to, NEAR or Soroban.
    pub fn from_release_tag(tag: &str) -> Option<Self> {
        artifact_from_release_tag(tag)
            .map(Self::Near)
            .or_else(|| SorobanArtifactId::from_release_tag(tag).map(Self::Soroban))
    }

    pub fn package_name(self) -> &'static str {
        match self {
            Self::Near(id) => id.metadata().package_name,
            Self::Soroban(id) => id.metadata().package_name,
        }
    }

    pub fn cargo_target_name(self) -> &'static str {
        match self {
            Self::Near(id) => id.metadata().cargo_target_name,
            Self::Soroban(id) => id.metadata().cargo_target_name,
        }
    }

    pub fn source_path(self) -> &'static str {
        match self {
            Self::Near(id) => id.metadata().source_path,
            Self::Soroban(id) => id.metadata().source_path,
        }
    }

    pub fn releases(self) -> &'static [ArtifactRelease] {
        match self {
            Self::Near(id) => id.metadata().releases(),
            Self::Soroban(id) => id.metadata().releases(),
        }
    }

    /// The one place a new release's asset gets its name; existing releases
    /// have theirs recorded in `releases/`.
    pub fn asset_name(self, version: impl fmt::Display) -> String {
        format!("{}-{version}.wasm", self.cargo_target_name())
    }
}

impl fmt::Display for ReleaseArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Near(id) => id.fmt(formatter),
            Self::Soroban(id) => id.fmt(formatter),
        }
    }
}

/// A catalog key as `record-release` and a `releases/` row spell it. NEAR
/// first, so no existing spelling can change meaning.
impl FromStr for ReleaseArtifact {
    type Err = ArtifactParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse()
            .map(Self::Near)
            .or_else(|_| s.parse().map(Self::Soroban))
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
static PATCH_STATE_METADATA: ArtifactMetadata = entry!(
    PatchState,
    "templar-patch-state-contract",
    "templar_patch_state_contract",
    "contract/patch-state"
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
static MOCK_SIGNER_METADATA: ArtifactMetadata =
    entry!(MockSigner, "mock-signer", "mock_signer", "mock/signer");
static MOCK_DAO_METADATA: ArtifactMetadata = entry!(MockDao, "mock-dao", "mock_dao", "mock/dao");

static SOROBAN_PROXY_ORACLE_METADATA: SorobanArtifactMetadata = SorobanArtifactMetadata {
    id: SorobanArtifactId::ProxyOracle,
    package_name: "templar-proxy-oracle-soroban-contract",
    cargo_target_name: "templar_proxy_oracle_soroban_contract",
    source_path: "contract/proxy-oracle/soroban/contract",
};
static SOROBAN_PROXY_GOVERNANCE_METADATA: SorobanArtifactMetadata = SorobanArtifactMetadata {
    id: SorobanArtifactId::ProxyGovernance,
    package_name: "templar-proxy-oracle-soroban-governance-contract",
    cargo_target_name: "templar_proxy_oracle_soroban_governance_contract",
    source_path: "contract/proxy-oracle/soroban/governance-contract",
};
static SOROBAN_SEP40_ADAPTER_METADATA: SorobanArtifactMetadata = SorobanArtifactMetadata {
    id: SorobanArtifactId::Sep40Adapter,
    package_name: "templar-proxy-oracle-soroban-sep40-adapter-contract",
    cargo_target_name: "templar_proxy_oracle_soroban_sep40_adapter_contract",
    source_path: "contract/proxy-oracle/soroban/sep40-adapter-contract",
};
static SOROBAN_LAZER_SOURCE_METADATA: SorobanArtifactMetadata = SorobanArtifactMetadata {
    id: SorobanArtifactId::LazerSource,
    package_name: "templar-proxy-oracle-soroban-pyth-lazer-source-contract",
    cargo_target_name: "templar_proxy_oracle_soroban_pyth_lazer_source_contract",
    source_path: "contract/proxy-oracle/soroban/pyth-lazer-source-contract",
};
static SOROBAN_BATCHER_METADATA: SorobanArtifactMetadata = SorobanArtifactMetadata {
    id: SorobanArtifactId::Batcher,
    package_name: "templar-proxy-oracle-soroban-batcher-contract",
    cargo_target_name: "templar_proxy_oracle_soroban_batcher_contract",
    source_path: "contract/proxy-oracle/soroban/batcher-contract",
};
