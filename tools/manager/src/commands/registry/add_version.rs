use std::path::PathBuf;

use anyhow::Context as _;
use clap::{ArgGroup, Args, ValueEnum};
use near_account_id::AccountId;
use near_sdk::json_types::Base58CryptoHash;
use templar_common::registry::VersionSource;
use templar_contract_artifacts::ArtifactId;
use templar_gateway_methods_spec::registry as spec;
use templar_gateway_types::NearToken;
use templar_tools_common::build::{build_contract, load_contract, LoadedContract};

use crate::commands::signer::SignerArgs;

/// Rough NEAR-per-byte storage staking rate used to size a global-hash upload
/// (matches the registry contract's own accounting).
pub(crate) const STORAGE_AMOUNT_PER_BYTE: NearToken =
    NearToken::from_yoctonear(10_000_000_000_000_000_000);

/// How to register a wasm blob. Local to the manager so `templar-common` keeps
/// clap gated behind its (non-default, host-only) `rpc` feature and never pulls
/// it into a wasm contract build.
///
/// The value names predate [`VersionSource`] and are kept as the CLI surface.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum DeployModeArg {
    Normal,
    GlobalHash,
}

impl DeployModeArg {
    fn with_code(self, code: Vec<u8>) -> VersionSource {
        match self {
            Self::Normal => VersionSource::Stored(code.into()),
            Self::GlobalHash => VersionSource::PublishGlobal(code.into()),
        }
    }
}

/// The known NEAR contracts, as shorthand values for `--contract`. Each maps to
/// an `ArtifactId` under `contract/*`, collapsing what used to be one boolean
/// flag per contract into a single validated value.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum ContractArg {
    Registry,
    Market,
    Vault,
    /// `uac` is kept as a legacy alias for the old shorthand flag.
    #[value(alias = "uac")]
    UniversalAccount,
    ProxyOracle,
    ProxyOracleGovernance,
    LstOracle,
    RedstoneAdapter,
    PythLazerAdapter,
    PatchState,
}

impl From<ContractArg> for ArtifactId {
    fn from(contract: ContractArg) -> Self {
        match contract {
            ContractArg::Registry => Self::Registry,
            ContractArg::Market => Self::Market,
            ContractArg::Vault => Self::Vault,
            ContractArg::UniversalAccount => Self::UniversalAccount,
            ContractArg::ProxyOracle => Self::ProxyOracle,
            ContractArg::ProxyOracleGovernance => Self::ProxyGovernance,
            ContractArg::LstOracle => Self::LstOracle,
            ContractArg::RedstoneAdapter => Self::RedstoneAdapter,
            ContractArg::PythLazerAdapter => Self::PythLazerAdapter,
            ContractArg::PatchState => Self::PatchState,
        }
    }
}

/// Where the WASM to register comes from, and how to identify it.
///
/// Exactly one contract selector is required: `--contract <CONTRACT>` (a known
/// NEAR contract), `--package` (a Cargo package name or artifact ID), `--wasm`
/// (an explicit file), or `--code-hash` (a global contract already on chain).
/// The `--contract`/`--package` modes resolve a workspace package and, by
/// default, build it reproducibly (`--no-build` uploads the last `target/near`
/// build instead); `--wasm` uploads arbitrary bytes and so requires
/// `--version-key`, as does `--code-hash`, which uploads nothing at all.
#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("contract_source")
        .args(["contract", "package", "wasm", "code_hash"])
        .required(true)
        .multiple(false)
))]
pub struct ContractSource {
    /// Known NEAR contract to build and register.
    #[arg(long, value_enum, value_name = "CONTRACT")]
    contract: Option<ContractArg>,
    /// Contract by Cargo package name or artifact ID (e.g. `market`)
    #[arg(long, visible_alias = "artifact", value_name = "NAME")]
    package: Option<String>,
    /// Upload this WASM file directly, skipping the build (requires --version-key)
    #[arg(long, value_name = "PATH")]
    wasm: Option<PathBuf>,
    /// Register a global contract already on chain by its code hash, uploading
    /// nothing (requires --version-key)
    #[arg(long, value_name = "HASH")]
    code_hash: Option<Base58CryptoHash>,
    /// Upload the last `target/near` build instead of building (package modes only)
    #[arg(long)]
    no_build: bool,
    /// Workspace root to build/load the package from
    #[arg(long, env = "WORKSPACE_PATH", default_value = ".", value_name = "PATH")]
    workspace_path: PathBuf,
}

impl ContractSource {
    fn artifact(&self) -> Option<ArtifactId> {
        self.contract
            .map(ArtifactId::from)
            .or_else(|| self.package.as_deref().and_then(|p| p.parse().ok()))
    }

    /// Resolve the bytes to upload and, when derivable, the canonical version
    /// key (`{name}@{version}#{sha256}`). `--wasm` yields no key — the caller
    /// must supply `--version-key`.
    fn load(&self) -> anyhow::Result<(Vec<u8>, Option<String>)> {
        if let Some(path) = &self.wasm {
            let bytes = std::fs::read(path)
                .with_context(|| format!("read WASM from {}", path.display()))?;
            return Ok((bytes, None));
        }

        let package_name = self
            .artifact()
            .map(|artifact| artifact.metadata().package_name.to_string())
            .or_else(|| self.package.clone())
            .context("no contract selected")?;

        let loaded: LoadedContract<()> = if self.no_build {
            load_contract(&self.workspace_path, &package_name)?
        } else {
            build_contract(&self.workspace_path, &package_name)?
        };
        let version_key = loaded.version_key();
        Ok((loaded.wasm_bytes, Some(version_key)))
    }
}

/// Build (or load) a contract and register it as a deployable version.
#[derive(Args, Debug)]
pub struct AddVersion {
    /// Registry to add the version to.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[command(flatten)]
    source: ContractSource,
    /// Version key to store. Derived from the package metadata and WASM hash
    /// when omitted; required with --wasm or --code-hash.
    #[arg(long, value_name = "KEY")]
    version_key: Option<String>,
    /// Deployment mode for an uploaded WASM (choose explicitly). Meaningless
    /// with --code-hash, which never uploads.
    #[arg(
        long,
        value_enum,
        required_unless_present = "code_hash",
        conflicts_with = "code_hash"
    )]
    deploy_mode: Option<DeployModeArg>,
    /// Deposit in NEAR. Estimated from the version source when omitted.
    #[arg(long, value_name = "AMOUNT")]
    deposit: Option<NearToken>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl AddVersion {
    pub fn try_into_spec(self) -> anyhow::Result<spec::AddVersion> {
        let (source, derived_key) = if let Some(hash) = self.source.code_hash {
            (VersionSource::ExistingGlobal(hash), None)
        } else {
            let deploy_mode = self
                .deploy_mode
                .context("--deploy-mode is required unless --code-hash is used")?;
            let (wasm_bytes, derived_key) = self.source.load()?;
            (deploy_mode.with_code(wasm_bytes), derived_key)
        };

        let version_key = self
            .version_key
            .or(derived_key)
            .context("--version-key is required when using --wasm or --code-hash")?;
        let deposit = self.deposit.unwrap_or_else(|| estimate_deposit(&source));

        Ok(spec::AddVersion {
            registry_id: self.registry_id,
            version_key,
            source,
            deposit,
        })
    }
}

/// Publishing a global contract stakes storage for the code itself. Storing pays
/// only the record, and registering a global that already exists stakes nothing at
/// all — both take the nominal yocto. Overridable with `--deposit`.
fn estimate_deposit(source: &VersionSource) -> NearToken {
    match source {
        VersionSource::PublishGlobal(code) => {
            STORAGE_AMOUNT_PER_BYTE.saturating_mul(code.0.len() as u128 * 10)
        }
        VersionSource::Stored(_) | VersionSource::ExistingGlobal(_) => NearToken::from_yoctonear(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        source: ContractSource,
    }

    fn source_for(value: &str) -> ContractSource {
        Harness::try_parse_from(["tmplrmgr", "--contract", value])
            .expect("--contract value should parse")
            .source
    }

    const CONTRACT_VALUES: [(&str, ArtifactId); 10] = [
        ("registry", ArtifactId::Registry),
        ("market", ArtifactId::Market),
        ("vault", ArtifactId::Vault),
        ("universal-account", ArtifactId::UniversalAccount),
        ("proxy-oracle", ArtifactId::ProxyOracle),
        ("proxy-oracle-governance", ArtifactId::ProxyGovernance),
        ("lst-oracle", ArtifactId::LstOracle),
        ("redstone-adapter", ArtifactId::RedstoneAdapter),
        ("pyth-lazer-adapter", ArtifactId::PythLazerAdapter),
        ("patch-state", ArtifactId::PatchState),
    ];

    #[test]
    fn each_contract_value_maps_to_its_artifact() {
        for (value, expected) in CONTRACT_VALUES {
            assert_eq!(
                source_for(value).artifact(),
                Some(expected),
                "value {value}"
            );
        }
    }

    #[test]
    fn uac_is_a_legacy_alias_for_universal_account() {
        assert_eq!(
            source_for("uac").artifact(),
            Some(ArtifactId::UniversalAccount)
        );
    }

    /// Guard: every NEAR contract under `contract/*` must have a `--contract`
    /// value, so a newly-added contract can't silently ship without one.
    #[test]
    fn every_contract_artifact_has_a_value() {
        for id in ArtifactId::ALL {
            if !id.metadata().source_path.starts_with("contract/") {
                continue;
            }
            assert!(
                CONTRACT_VALUES.iter().any(|&(_, mapped)| mapped == id),
                "{id:?} lives under contract/* but has no --contract value",
            );
        }
    }

    fn code(len: usize) -> near_sdk::json_types::Base64VecU8 {
        vec![0u8; len].into()
    }

    #[test]
    fn estimate_deposit_matches_storage_staking_math() {
        // Stored pays a nominal 1 yocto; PublishGlobal stakes storage for the
        // code at 1e19 yocto/byte times the 10x global-contract multiplier.
        assert_eq!(
            estimate_deposit(&VersionSource::Stored(code(100_000))),
            NearToken::from_yoctonear(1)
        );
        assert_eq!(
            estimate_deposit(&VersionSource::PublishGlobal(code(0))),
            NearToken::from_yoctonear(0)
        );
        assert_eq!(
            estimate_deposit(&VersionSource::PublishGlobal(code(100_000))),
            NearToken::from_near(10)
        );
    }

    /// Registering an existing global stakes nothing, so the byte-length estimate must not apply —
    /// that is the entire point of the code-hash source.
    #[test]
    fn estimate_deposit_for_a_code_hash_is_nominal() {
        assert_eq!(
            estimate_deposit(&VersionSource::ExistingGlobal(Base58CryptoHash::from(
                [7u8; 32]
            ))),
            NearToken::from_yoctonear(1),
        );
    }
}
