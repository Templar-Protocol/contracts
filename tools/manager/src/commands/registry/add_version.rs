use std::path::PathBuf;

use anyhow::Context as _;
use clap::{ArgGroup, Args, ValueEnum};
use near_account_id::AccountId;
use templar_common::registry::DeployMode;
use templar_contract_artifacts::ArtifactId;
use templar_gateway_methods_spec::registry as spec;
use templar_gateway_types::{Base64Bytes, NearToken};
use templar_tools_common::build::{build_contract, load_contract, LoadedContract};

use crate::commands::signer::SignerArgs;

/// Rough NEAR-per-byte storage staking rate used to size a global-hash upload
/// (matches the registry contract's own accounting).
const STORAGE_AMOUNT_PER_BYTE: NearToken = NearToken::from_yoctonear(10_000_000_000_000_000_000);

/// CLI mirror of [`DeployMode`]. Local to the manager so `templar-common` keeps
/// clap gated behind its (non-default, host-only) `rpc` feature and never pulls
/// it into a wasm contract build.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum DeployModeArg {
    Normal,
    GlobalHash,
}

impl From<DeployModeArg> for DeployMode {
    fn from(mode: DeployModeArg) -> Self {
        match mode {
            DeployModeArg::Normal => Self::Normal,
            DeployModeArg::GlobalHash => Self::GlobalHash,
        }
    }
}

/// Where the WASM to register comes from, and how to identify it.
///
/// Exactly one contract selector is required: a shortcut flag (one per NEAR
/// contract under `contract/*`), `--package` (a Cargo package name or artifact
/// ID), or `--wasm` (an explicit file). The shortcut/`--package` modes resolve
/// a workspace package and, by default, build it reproducibly (`--no-build`
/// uploads the last `target/near` build instead); `--wasm` uploads arbitrary
/// bytes and so requires `--version-key`.
#[allow(clippy::struct_excessive_bools)] // one bool per contract shortcut flag
#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("contract_source")
        .args([
            "registry", "market", "vault", "uac", "proxy_oracle", "proxy_governance",
            "lst_oracle", "redstone_adapter", "pyth_lazer_adapter", "package", "wasm",
        ])
        .required(true)
        .multiple(false)
))]
pub struct ContractSource {
    /// Registry contract
    #[arg(long)]
    registry: bool,
    /// Market contract
    #[arg(long)]
    market: bool,
    /// Vault contract
    #[arg(long)]
    vault: bool,
    /// Universal account contract
    #[arg(long)]
    uac: bool,
    /// Proxy oracle contract
    #[arg(long)]
    proxy_oracle: bool,
    /// Proxy oracle governance contract
    #[arg(long)]
    proxy_governance: bool,
    /// LST oracle contract
    #[arg(long)]
    lst_oracle: bool,
    /// RedStone adapter contract
    #[arg(long)]
    redstone_adapter: bool,
    /// Pyth Lazer adapter contract
    #[arg(long)]
    pyth_lazer_adapter: bool,
    /// Contract by Cargo package name or artifact ID (e.g. `market`)
    #[arg(long, visible_alias = "artifact", value_name = "NAME")]
    package: Option<String>,
    /// Upload this WASM file directly, skipping the build (requires --version-key)
    #[arg(long, value_name = "PATH")]
    wasm: Option<PathBuf>,
    /// Upload the last `target/near` build instead of building (package modes only)
    #[arg(long)]
    no_build: bool,
    /// Workspace root to build/load the package from
    #[arg(long, env = "WORKSPACE_PATH", default_value = ".", value_name = "PATH")]
    workspace_path: PathBuf,
}

impl ContractSource {
    fn artifact(&self) -> Option<ArtifactId> {
        [
            (self.registry, ArtifactId::Registry),
            (self.market, ArtifactId::Market),
            (self.vault, ArtifactId::Vault),
            (self.uac, ArtifactId::UniversalAccount),
            (self.proxy_oracle, ArtifactId::ProxyOracle),
            (self.proxy_governance, ArtifactId::ProxyGovernance),
            (self.lst_oracle, ArtifactId::LstOracle),
            (self.redstone_adapter, ArtifactId::RedstoneAdapter),
            (self.pyth_lazer_adapter, ArtifactId::PythLazerAdapter),
        ]
        .into_iter()
        .find_map(|(selected, id)| selected.then_some(id))
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
            .map(|a| a.metadata().package_name.to_string())
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
    /// when omitted; required with --wasm.
    #[arg(long, value_name = "KEY")]
    version_key: Option<String>,
    /// Deployment mode
    #[arg(long, value_enum, default_value = "normal")]
    deploy_mode: DeployModeArg,
    /// Deposit in NEAR. Estimated from the WASM size and deploy mode when omitted.
    #[arg(long, value_name = "AMOUNT")]
    deposit: Option<NearToken>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl AddVersion {
    pub fn try_into_spec(self) -> anyhow::Result<spec::AddVersion> {
        let deploy_mode = DeployMode::from(self.deploy_mode);
        let (wasm_bytes, derived_key) = self.source.load()?;
        let version_key = self
            .version_key
            .or(derived_key)
            .context("--version-key is required when using --wasm")?;
        let deposit = self
            .deposit
            .unwrap_or_else(|| estimate_deposit(deploy_mode, wasm_bytes.len()));

        Ok(spec::AddVersion {
            registry_id: self.registry_id,
            version_key,
            deploy_mode,
            code: Base64Bytes(wasm_bytes),
            deposit,
        })
    }
}

/// Global-hash uploads stake storage for the code itself; a plain deploy pays
/// only the record. Overridable with `--deposit`.
fn estimate_deposit(mode: DeployMode, wasm_len: usize) -> NearToken {
    match mode {
        DeployMode::GlobalHash => STORAGE_AMOUNT_PER_BYTE.saturating_mul(wasm_len as u128 * 10),
        DeployMode::Normal => NearToken::from_yoctonear(1),
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

    fn source_for(flag: &str) -> ContractSource {
        Harness::try_parse_from(["tmplrmgr", flag])
            .expect("shortcut flag should parse")
            .source
    }

    #[test]
    fn each_contract_shortcut_maps_to_its_artifact() {
        let cases = [
            ("--registry", ArtifactId::Registry),
            ("--market", ArtifactId::Market),
            ("--vault", ArtifactId::Vault),
            ("--uac", ArtifactId::UniversalAccount),
            ("--proxy-oracle", ArtifactId::ProxyOracle),
            ("--proxy-governance", ArtifactId::ProxyGovernance),
            ("--lst-oracle", ArtifactId::LstOracle),
            ("--redstone-adapter", ArtifactId::RedstoneAdapter),
            ("--pyth-lazer-adapter", ArtifactId::PythLazerAdapter),
        ];
        for (flag, expected) in cases {
            assert_eq!(source_for(flag).artifact(), Some(expected), "flag {flag}");
        }
    }

    /// Guard: every NEAR contract under `contract/*` must have a shortcut, so a
    /// newly-added contract can't silently ship without one.
    #[test]
    fn every_contract_artifact_has_a_shortcut() {
        for id in ArtifactId::ALL {
            if id.metadata().source_path.starts_with("contract/") {
                let flag = match id {
                    ArtifactId::Registry => "--registry",
                    ArtifactId::Market => "--market",
                    ArtifactId::Vault => "--vault",
                    ArtifactId::UniversalAccount => "--uac",
                    ArtifactId::ProxyOracle => "--proxy-oracle",
                    ArtifactId::ProxyGovernance => "--proxy-governance",
                    ArtifactId::LstOracle => "--lst-oracle",
                    ArtifactId::RedstoneAdapter => "--redstone-adapter",
                    ArtifactId::PythLazerAdapter => "--pyth-lazer-adapter",
                    other => {
                        panic!("{other:?} lives under contract/* but has no add-version shortcut")
                    }
                };
                assert_eq!(source_for(flag).artifact(), Some(id));
            }
        }
    }

    #[test]
    fn estimate_deposit_matches_storage_staking_math() {
        // Normal deploys pay a nominal 1 yocto; GlobalHash stakes storage for the
        // code at 1e19 yocto/byte times the 10x global-contract multiplier.
        assert_eq!(
            estimate_deposit(DeployMode::Normal, 100_000),
            NearToken::from_yoctonear(1)
        );
        assert_eq!(
            estimate_deposit(DeployMode::GlobalHash, 0),
            NearToken::from_yoctonear(0)
        );
        assert_eq!(
            estimate_deposit(DeployMode::GlobalHash, 100_000),
            NearToken::from_near(10)
        );
    }
}
