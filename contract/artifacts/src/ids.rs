//! Canonical contract artifact identifiers and metadata.
//!
//! Every contract that can be deployed, tested, or managed through the
//! gateway / manager / test-utils toolchain is listed here. The catalog is
//! the single source of truth for artifact names, paths, and how they map
//! to `target/near` directories.
//!
//! ⚠️ Each entry's `releases` list is the *released* history of a contract, NOT
//! a mirror of current source. Source is allowed to move ahead of the newest
//! release — unreleased work-in-progress is meant to lag it.
//!
//! Bytes live on the GitHub Release for each version's tag, not in this repo;
//! [`crate::fetch`] downloads and caches them. Each release pins the SHA-256 of
//! its bytes, so a swapped asset is caught locally rather than trusted.
//!
//! Releases are **immutable**: ship new bytes by bumping the contract's
//! `Cargo.toml` version and *adding* a release, never by editing one. Historical
//! releases are what the migration and upgrade tests deploy, so rewriting one
//! silently invalidates them.
//!
//! Cutting a release is not a manual step: merging the release PR tags the
//! version, and `.github/workflows/release-artifacts.yml` builds the WASM
//! reproducibly at that tag, uploads it, and opens the PR that fills in the
//! pin. See `RELEASING.md`.

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
/// A release exists because the bytes were *deployed*, not because a version
/// number was bumped: those diverge, and this catalog tracks the former. It is
/// appended to by CI when a release tag is cut, never by hand — see the module
/// docs for why immutability matters here.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct ArtifactRelease {
    /// Crate version this was released as.
    pub version: &'static str,
    /// SHA-256 (lowercase hex) of the released bytes.
    ///
    /// The root of trust for a downloaded asset: [`crate::fetch`] verifies
    /// against it and refuses bytes that do not match, so artifact integrity
    /// rests on this reviewed, in-repo value rather than on GitHub serving us
    /// the right file.
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
    /// Empty for mock contracts: they are test scaffolding, never tagged and
    /// never deployed, so "the canonical bytes of mock-ft" is not a thing that
    /// exists. Tests build them from source instead.
    pub releases: &'static [ArtifactRelease],
}

impl ArtifactMetadata {
    pub fn manifest_path(&self) -> std::path::PathBuf {
        Path::new(self.source_path).join("Cargo.toml")
    }

    /// The newest release — what the gateway deploys and what `version_key`
    /// refers to by default. `None` for an artifact that has never been
    /// released (mocks).
    pub fn current(&self) -> Option<&'static ArtifactRelease> {
        self.releases.last()
    }

    /// Version of the newest release.
    pub fn version(&self) -> Option<&'static str> {
        self.current().map(|release| release.version)
    }

    /// Look up a specific released version.
    pub fn release(&self, version: &str) -> Option<&'static ArtifactRelease> {
        self.releases.iter().find(|r| r.version == version)
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
    ($id:ident, $pkg:expr, $target:expr, $src:expr, [$(($ver:expr, $sha:expr)),* $(,)?]) => {
        ArtifactMetadata {
            id: ArtifactId::$id,
            package_name: $pkg,
            cargo_target_name: $target,
            source_path: $src,
            releases: &[$(ArtifactRelease { version: $ver, sha256: $sha }),*],
        }
    };
}

// Every release below was recovered from code actually deployed on NEAR
// mainnet, and its tag points at the commit that deployed WASM names in its
// NEP-330 metadata — so each one rebuilds byte-for-byte from a fresh clone.
// Each release's GitHub Release names the account its bytes were read from.
//
// `source_path` is where the contract lives *today*. Older releases were built
// from different paths (proxy-oracle from `contract/proxy-oracle`, the LST
// oracle from `contract/lst-oracle`); a verifier reads the historical path out
// of the WASM's own build_info, so this field only describes current work.

static REGISTRY_METADATA: ArtifactMetadata = entry!(
    Registry,
    "templar-registry-contract",
    "templar_registry_contract",
    "contract/registry",
    [
        (
            "0.1.0",
            "c07b38a29202da247da12515d3b2e7a93855619209a76d73e5ae12b2facc6255"
        ),
        (
            "1.0.0",
            "bafbec54bff4f07147c5e317966ec34c500331f02c64eefd5da1aa5dc778e339"
        ),
        (
            "1.1.0",
            "0f315a008969013f44575b16374c115e972f421d1061fd6cafc8b4f63e5c65f3"
        ),
    ]
);
static MARKET_METADATA: ArtifactMetadata = entry!(
    Market,
    "templar-market-contract",
    "templar_market_contract",
    "contract/market",
    [
        (
            "1.0.0",
            "0b78aefdae1bb76148602252dd6b7db2014d99ef3b7f93a764c54d7ebe97a392"
        ),
        (
            "1.1.0",
            "2bc028a722ccacd8aa23fe50a83c395716c920bf724b0d054a1ede3f2e4fae77"
        ),
        (
            "1.2.1",
            "c9752de38ed140645e9a8d4a55dc89835b113609e27bb63ce134b5e598bc69b8"
        ),
        (
            "1.3.0",
            "40e56aaf3627f1cb13656e696d382e64d7bb3e97420a049c20ae294d4f64fe97"
        ),
    ]
);

// No NEAR vault has been deployed yet; the Soroban vault is a separate
// artifact. CI appends the first NEAR release here when one ships.
static VAULT_METADATA: ArtifactMetadata = entry!(
    Vault,
    "templar-vault-contract",
    "templar_vault_contract",
    "contract/vault/near",
    []
);

// 0.4.0 is deliberately absent: it was built and used as a migration-test
// fixture but never deployed, so it is not a release. Those bytes live beside
// the state patch they pair with, in contract/universal-account/tests/migration/.
static UNIVERSAL_ACCOUNT_METADATA: ArtifactMetadata = entry!(
    UniversalAccount,
    "templar-universal-account-contract",
    "templar_universal_account_contract",
    "contract/universal-account",
    [
        (
            "0.2.0",
            "30e8baf1bd5de0308c510c0e5ff77b9aeb25306bf6ab922e5e25f8a24d6f3efd"
        ),
        (
            "0.3.0",
            "99e7ee1e559b2a0e966ef91e572007d71a6998b6a8d8ca0330e6c308652de505"
        ),
        (
            "0.3.1",
            "1b470283a0393f083463c95086de3046a94b626de45270fc108c1658fc7679a0"
        ),
        (
            "0.5.0",
            "22d1efca17929ef7148f62b9a32f1dcf9a5aa6bf1646df2190817e8cb9e961ef"
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
            "e877687e2d6f51db824bde12348938b3374f526301811df3ee118af38b856f35"
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
        "1.0.0",
        "4d650b07ec9ccc3aaafc765f6970e2c60dde1ba23f25ef05d23ddd2178a157a3"
    ),]
);
static REDSTONE_ADAPTER_METADATA: ArtifactMetadata = entry!(
    RedstoneAdapter,
    "templar-redstone-adapter-contract",
    "templar_redstone_adapter_contract",
    "contract/redstone-adapter",
    [(
        "0.1.0",
        "debbf6f631422ad44fb3bf323503f378cdaff1cd6c63661846cb56828e23c461"
    ),]
);
static PYTH_LAZER_ADAPTER_METADATA: ArtifactMetadata = entry!(
    PythLazerAdapter,
    "templar-pyth-lazer-adapter-contract",
    "templar_pyth_lazer_adapter_contract",
    "contract/pyth-lazer/contract",
    [(
        "0.1.0",
        "ff51847ea292271775db9bf8207e7f30c5a6a426b277a71ebe78f8c4850a451f"
    ),]
);

// Mocks are test scaffolding: Tier C in `release-plz.toml`, never tagged, never
// deployed. Tests build them from source.
static MOCK_FT_METADATA: ArtifactMetadata = entry!(MockFt, "mock-ft", "mock_ft", "mock/ft", []);
static MOCK_MT_METADATA: ArtifactMetadata = entry!(MockMt, "mock-mt", "mock_mt", "mock/mt", []);
static MOCK_ORACLE_METADATA: ArtifactMetadata =
    entry!(MockOracle, "mock-oracle", "mock_oracle", "mock/oracle", []);
static MOCK_REF_FINANCE_METADATA: ArtifactMetadata =
    entry!(MockRefFinance, "mock-ref", "mock_ref", "mock/ref", []);
static MOCK_RECEIVER_METADATA: ArtifactMetadata = entry!(
    MockReceiver,
    "mock-receiver",
    "mock_receiver",
    "mock/receiver",
    []
);
