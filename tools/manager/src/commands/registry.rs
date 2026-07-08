use std::path::PathBuf;

use anyhow::Context as _;
use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use near_account_id::AccountId;
use near_api::PublicKey as CliPublicKey;
use templar_common::registry::DeployMode;
use templar_contract_artifacts::ArtifactId;
use templar_gateway_methods_spec::registry as spec;
use templar_gateway_types::{common::Pagination, Base64Bytes, ContractKind, NearToken};
use templar_tools_common::build::{build_contract, load_contract, LoadedContract};

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum RegistryNs {
    ListVersions(ListVersions),
    ListDeployments(ListDeployments),
    ListDeploymentsByKind(ListDeploymentsByKind),
    GetDeployment(GetDeployment),
    AddVersion(AddVersion),
    Deploy(Deploy),
    /// Remove a single version, or every version with `--all`.
    RemoveVersion(RemoveVersion),
    /// Remove every version from the registry, then delete the (signer) account.
    Remove(Remove),
    /// Remove every market deployed from the registry (signing as each with the
    /// shared `--secret-key`).
    ClearDeployments(ClearDeployments),
}

#[derive(Args, Debug)]
pub struct ListVersions {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    limit: Option<u32>,
}

impl ListVersions {
    pub fn parse(self) -> spec::ListVersions {
        spec::ListVersions {
            registry_id: self.registry_id,
            args: Pagination {
                offset: self.offset,
                limit: self.limit,
            },
        }
    }
}

#[derive(Args, Debug)]
pub struct ListDeployments {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    limit: Option<u32>,
}

impl ListDeployments {
    pub fn parse(self) -> spec::ListDeployments {
        spec::ListDeployments {
            registry_id: self.registry_id,
            args: Pagination {
                offset: self.offset,
                limit: self.limit,
            },
        }
    }
}

#[derive(Args, Debug)]
pub struct ListDeploymentsByKind {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long, value_enum)]
    kind: ContractKind,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    limit: Option<u32>,
}

impl ListDeploymentsByKind {
    pub fn parse(self) -> spec::ListDeploymentsByKind {
        spec::ListDeploymentsByKind {
            registry_id: self.registry_id,
            args: Pagination {
                offset: self.offset,
                limit: self.limit,
            },
            kind: self.kind,
        }
    }
}

#[derive(Args, Debug)]
pub struct GetDeployment {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
}

impl GetDeployment {
    pub fn parse(self) -> spec::GetDeployment {
        spec::GetDeployment {
            registry_id: self.registry_id,
            account_id: self.account_id,
        }
    }
}

#[derive(Args, Debug)]
pub struct Deploy {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long, value_name = "NAME")]
    name: String,
    #[arg(long, value_name = "KEY")]
    version_key: String,
    #[arg(long, value_name = "PATH")]
    init_args_file: Option<std::path::PathBuf>,
    /// Additional full access keys for the new account. The signer's key is
    /// added by default (unless `--no-signer-full-access-key`).
    #[arg(long = "with-full-access-key", value_name = "PUBLIC_KEY")]
    with_full_access_key: Vec<CliPublicKey>,
    /// Do not grant the signer's public key a full access key on the new account.
    #[arg(long)]
    no_signer_full_access_key: bool,
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

impl Deploy {
    /// The full-access-key flags: whether to omit the signer's key, and any
    /// extra keys to add. Resolved against the signer's public key at dispatch.
    pub fn full_access_key_flags(&self) -> (bool, Vec<CliPublicKey>) {
        (
            self.no_signer_full_access_key,
            self.with_full_access_key.clone(),
        )
    }

    pub fn into_spec(
        self,
        full_access_keys: Vec<templar_gateway_types::primitive::PublicKey>,
    ) -> anyhow::Result<spec::Deploy> {
        let init_bytes = match self.init_args_file {
            Some(path) => std::fs::read(&path)
                .map_err(anyhow::Error::from)
                .with_context(|| format!("read init args from {}", path.display()))?,
            None => b"null".to_vec(),
        };

        Ok(spec::Deploy {
            registry_id: self.registry_id,
            name: self.name,
            version_key: self.version_key,
            init_args: Base64Bytes(init_bytes),
            full_access_keys: Some(full_access_keys),
            deposit: self.deposit,
        })
    }
}

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("which_version").args(["version_key", "all"]).required(true)
))]
pub struct RemoveVersion {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Version key to remove. Omit and pass `--all` to remove every version.
    #[arg(long, value_name = "KEY")]
    version_key: Option<String>,
    /// Remove every version currently in the registry.
    #[arg(long)]
    all: bool,
}

impl RemoveVersion {
    pub fn registry_id(&self) -> &AccountId {
        &self.registry_id
    }

    /// The single-version spec, or `None` when `--all` was requested (the
    /// dispatcher then lists and removes each version).
    pub fn single(&self) -> Option<spec::RemoveVersion> {
        self.version_key
            .clone()
            .map(|version_key| spec::RemoveVersion {
                registry_id: self.registry_id.clone(),
                version_key,
            })
    }

    pub fn spec_for(&self, version_key: String) -> spec::RemoveVersion {
        spec::RemoveVersion {
            registry_id: self.registry_id.clone(),
            version_key,
        }
    }
}

#[derive(Args, Debug)]
pub struct Remove {
    /// Account to receive the registry account's remaining balance.
    #[arg(long, value_name = "ACCOUNT_ID")]
    beneficiary_id: AccountId,
}

impl Remove {
    pub fn beneficiary_id(&self) -> &AccountId {
        &self.beneficiary_id
    }
}

#[derive(Args, Debug)]
pub struct ClearDeployments {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Recovered assets and balances are sent here (defaults to the registry).
    #[arg(long, value_name = "ACCOUNT_ID")]
    beneficiary_id: Option<AccountId>,
    /// Continue past a market that fails to remove instead of stopping.
    #[arg(long)]
    force: bool,
}

impl ClearDeployments {
    pub fn registry_id(&self) -> &AccountId {
        &self.registry_id
    }

    /// Beneficiary for recovered funds, defaulting to the registry account.
    pub fn beneficiary_id(&self) -> AccountId {
        self.beneficiary_id
            .clone()
            .unwrap_or_else(|| self.registry_id.clone())
    }

    pub fn force(&self) -> bool {
        self.force
    }
}

/// Resolve the full access keys granted to an account deployed from a registry:
/// the signer's key by default (so the operator retains control), unless
/// suppressed, plus any explicitly-provided keys, de-duplicated.
pub fn resolve_full_access_keys(
    signer_public_key: templar_gateway_types::primitive::PublicKey,
    no_signer: bool,
    extra: &[CliPublicKey],
) -> Vec<templar_gateway_types::primitive::PublicKey> {
    let mut keys = Vec::with_capacity(extra.len() + 1);
    if !no_signer {
        keys.push(signer_public_key);
    }
    for key in extra {
        let key = templar_gateway_types::primitive::PublicKey::from(*key);
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

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
}

impl AddVersion {
    pub fn into_spec(self) -> anyhow::Result<spec::AddVersion> {
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

    const KEY_A: &str = "ed25519:5ZyGnGUdSp1pj7BbjTyWBvUyC2nh4RdkZyTphUYG4c4v";
    const KEY_B: &str = "ed25519:5TMKtTtD5uuMF28ovo7vVge7oAu58eXjySJWTrwcEB5w";

    fn pubkey(s: &str) -> CliPublicKey {
        s.parse().expect("valid ed25519 public key")
    }

    fn primitive(s: &str) -> templar_gateway_types::primitive::PublicKey {
        templar_gateway_types::primitive::PublicKey::from(pubkey(s))
    }

    #[test]
    fn fak_includes_signer_key_by_default() {
        // The signer's key is granted a full access key on every from-registry
        // deploy — the operator must retain control of the new account.
        let keys = resolve_full_access_keys(primitive(KEY_A), false, &[]);
        assert_eq!(keys, vec![primitive(KEY_A)]);
    }

    #[test]
    fn fak_no_signer_flag_drops_signer_key() {
        assert!(resolve_full_access_keys(primitive(KEY_A), true, &[]).is_empty());
        // With extra keys and --no-signer, only the extras are granted.
        let keys = resolve_full_access_keys(primitive(KEY_A), true, &[pubkey(KEY_B)]);
        assert_eq!(keys, vec![primitive(KEY_B)]);
    }

    #[test]
    fn fak_appends_and_dedups_extra_keys() {
        let keys =
            resolve_full_access_keys(primitive(KEY_A), false, &[pubkey(KEY_B), pubkey(KEY_A)]);
        // Signer key first, then the distinct extra; the duplicate is dropped.
        assert_eq!(keys, vec![primitive(KEY_A), primitive(KEY_B)]);
    }

    #[test]
    fn registry_deploy_grants_signer_and_extra_faks() {
        let cli = crate::cli::Cli::try_parse_from([
            "tmplrmgr",
            "registry",
            "deploy",
            "--registry-id",
            "registry.testnet",
            "--name",
            "market",
            "--version-key",
            "market@1",
            "--with-full-access-key",
            KEY_B,
            "--deposit",
            "6 NEAR",
        ])
        .expect("registry deploy should parse");
        let RegistryNs::Deploy(cmd) = (match cli.command {
            crate::cli::Command::Registry { command } => command,
            _ => panic!("expected registry"),
        }) else {
            panic!("expected deploy");
        };
        let (no_signer, extra) = cmd.full_access_key_flags();
        assert!(!no_signer);
        let keys = resolve_full_access_keys(primitive(KEY_A), no_signer, &extra);
        assert_eq!(keys, vec![primitive(KEY_A), primitive(KEY_B)]);
        let spec = cmd.into_spec(keys).expect("into spec");
        assert_eq!(spec.full_access_keys.expect("some").len(), 2);
    }

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
}
