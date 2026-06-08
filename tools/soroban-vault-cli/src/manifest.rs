use std::{
    collections::BTreeMap,
    fs,
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
    pub fn new(
        network: impl Into<String>,
        rpc_url: Option<String>,
        source_account: String,
    ) -> Self {
        Self {
            version: MANIFEST_VERSION,
            network: network.into(),
            rpc_url,
            source_account: Some(source_account),
            artifacts: BTreeMap::new(),
            contracts: BTreeMap::new(),
            transactions: Vec::new(),
        }
    }

    pub fn load_or_new(
        path: &Path,
        network: &str,
        rpc_url: Option<String>,
        source_account: String,
    ) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::new(network, rpc_url, source_account));
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
        if manifest.network.is_empty() {
            manifest.network = network.to_string();
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
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("manifest.json");
        let mut manifest = Manifest::new(
            "testnet",
            Some("https://rpc".to_string()),
            "alice".to_string(),
        );
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

        let loaded =
            Manifest::load_or_new(&path, "testnet", None, "alice".to_string()).expect("load");
        assert_eq!(loaded.version, MANIFEST_VERSION);
        assert!(loaded.artifacts.contains_key("vault"));
    }
}
