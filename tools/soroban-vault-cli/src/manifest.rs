use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

pub const MANIFEST_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub network: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpc_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_account: Option<String>,
    #[serde(default)]
    pub artifacts: BTreeMap<String, ArtifactRecord>,
    #[serde(default)]
    pub contracts: BTreeMap<String, ContractRecord>,
    #[serde(default)]
    pub transactions: Vec<TransactionRecord>,
}

impl Manifest {
    pub fn new(network: impl Into<String>, rpc_url: Option<String>) -> Self {
        Self {
            version: MANIFEST_VERSION,
            network: network.into(),
            rpc_url,
            source_account: None,
            artifacts: BTreeMap::new(),
            contracts: BTreeMap::new(),
            transactions: Vec::new(),
        }
    }

    pub fn load_or_new(
        path: &Path,
        network: &str,
        rpc_url: Option<String>,
    ) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::new(network, rpc_url));
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read manifest {}", path.display()))?;
        let mut manifest: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parse manifest {}", path.display()))?;
        anyhow::ensure!(
            manifest.version == MANIFEST_VERSION,
            "unsupported manifest version {}",
            manifest.version
        );
        anyhow::ensure!(
            !manifest.network.is_empty(),
            "manifest {} has no network provenance; use a new --state path",
            path.display()
        );
        anyhow::ensure!(
            manifest.network == network,
            "manifest network mismatch: recorded {}, requested {network}",
            manifest.network
        );
        // Scrub legacy or hand-edited source account data; signing identity
        // must stay in env/keystore.
        manifest.source_account = None;
        Ok(manifest)
    }

    pub fn create_new(path: &Path, network: &str, rpc_url: Option<String>) -> anyhow::Result<Self> {
        let manifest = Self::new(network, rpc_url);
        let raw = serde_json::to_vec_pretty(&manifest)?;
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("create manifest directory {}", parent.display()))?;
        }
        let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                anyhow::bail!(
                    "fresh deployment requires an unused --state path; {} already exists",
                    path.display()
                )
            }
            Err(error) => {
                return Err(error).with_context(|| format!("create manifest {}", path.display()));
            }
        };
        if let Err(error) = file.write_all(&raw) {
            let _ = fs::remove_file(path);
            return Err(error).with_context(|| format!("write manifest {}", path.display()));
        }
        Ok(manifest)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create manifest directory {}", parent.display()))?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        fs::write(path, raw).with_context(|| format!("write manifest {}", path.display()))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub package: String,
    pub wasm_path: PathBuf,
    pub local_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_wasm_hash: Option<String>,
    pub verified: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContractRecord {
    pub contract_id: String,
    pub wasm_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
    #[serde(default)]
    pub constructor_args: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy_tx: Option<String>,
    pub initialized: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionRecord {
    #[serde(default)]
    pub timestamp_unix_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_public_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_hash: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("manifest.json");
        let mut manifest = Manifest::new("testnet", Some("https://rpc".to_string()));
        manifest.artifacts.insert(
            "vault".to_string(),
            ArtifactRecord {
                package: "pkg".to_string(),
                wasm_path: "target/pkg.wasm".into(),
                local_hash: "abc".to_string(),
                remote_wasm_hash: Some("abc".to_string()),
                verified: true,
            },
        );
        manifest.save(&path).expect("save");

        let loaded = Manifest::load_or_new(&path, "testnet", None).expect("load");
        assert_eq!(loaded.version, MANIFEST_VERSION);
        assert!(loaded.artifacts.contains_key("vault"));
        assert_eq!(loaded.source_account, None);
    }

    #[test]
    fn load_rejects_missing_or_mismatched_network_provenance() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("manifest.json");
        let mut manifest = Manifest::new("", None);
        manifest.save(&path).expect("save empty network manifest");

        let error = Manifest::load_or_new(&path, "testnet", None)
            .expect_err("empty network provenance must fail");
        assert!(error.to_string().contains("has no network provenance"));

        manifest.network = "mainnet".to_string();
        manifest.save(&path).expect("save mismatched manifest");
        let error = Manifest::load_or_new(&path, "testnet", None)
            .expect_err("mismatched network must fail");
        assert!(error.to_string().contains("manifest network mismatch"));
    }

    #[test]
    fn create_new_reserves_manifest_path_exclusively() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/manifest.json");

        let manifest = Manifest::create_new(&path, "testnet", None).expect("create manifest");
        let raw = fs::read_to_string(&path).expect("read reserved manifest");
        let stored: Manifest = serde_json::from_str(&raw).expect("parse reserved manifest");

        assert_eq!(manifest.network, "testnet");
        assert_eq!(stored.network, "testnet");
        assert!(stored.contracts.is_empty());

        let error =
            Manifest::create_new(&path, "testnet", None).expect_err("second reservation must fail");
        assert!(error
            .to_string()
            .contains("fresh deployment requires an unused --state path"));
    }
}
