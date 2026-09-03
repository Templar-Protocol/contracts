use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    error::{Error, Result},
    output::CommandData,
    state::RouteStore,
};

const LOCK_BYTES: &[u8] = include_bytes!("../artifacts.lock.json");

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArtifactLockV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub layerzero_source: LayerZeroSourceLockV1,
    pub stellar: StellarArtifactLockV1,
    pub evm: EvmArtifactLockV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LayerZeroSourceLockV1 {
    pub remote: String,
    pub commit: String,
    pub source_archive_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StellarArtifactLockV1 {
    pub rust_toolchain: String,
    pub target: String,
    pub soroban_cli: String,
    pub oft_wasm_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvmArtifactLockV1 {
    pub oft_evm_version: String,
    pub solc: String,
    pub optimizer: bool,
    pub optimizer_runs: u32,
    pub wrapper_source_sha256: String,
    pub package_json_sha256: String,
    pub creation_bytecode_sha256: Option<String>,
    pub runtime_bytecode_sha256: Option<String>,
}

pub fn embedded_lock() -> Result<ArtifactLockV1> {
    let lock: ArtifactLockV1 = serde_json::from_slice(LOCK_BYTES)?;
    if lock.schema_name != "artifact_lock" || lock.schema_version != 1 {
        return Err(Error::InvalidInput(
            "unsupported artifact lock schema".into(),
        ));
    }
    if lock.evm.oft_evm_version != "4.0.1"
        || lock.evm.oft_evm_version.contains(['^', '~', '*', '>', '<'])
    {
        return Err(Error::Custody(
            "EVM OFT dependency must be pinned exactly to 4.0.1".into(),
        ));
    }
    Ok(lock)
}

pub fn verify_command(state: &Path) -> Result<CommandData> {
    let _ = RouteStore::open(state)?;
    let lock = embedded_lock()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    verify_hash(
        &root.join("evm/src/DisposableOFT.sol"),
        &lock.evm.wrapper_source_sha256,
    )?;
    verify_hash(
        &root.join("evm/package.json"),
        &lock.evm.package_json_sha256,
    )?;
    if lock.layerzero_source.source_archive_sha256.is_none()
        || lock.evm.creation_bytecode_sha256.is_none()
        || lock.evm.runtime_bytecode_sha256.is_none()
    {
        return Err(Error::Custody(
            "artifact qualification incomplete: source archive and built EVM bytecode hashes are not frozen".into(),
        ));
    }
    Ok(CommandData {
        result: serde_json::json!({"verified": true, "artifact_lock_sha256": sha256(LOCK_BYTES)}),
        artifact: None,
    })
}

pub fn build_command(state: &Path, out_dir: &Path, write: bool) -> Result<CommandData> {
    let route = RouteStore::open(state)?.load_state()?;
    crate::environment::require_testnet(&route.identity)?;
    let lock = embedded_lock()?;
    if !write {
        return Ok(CommandData {
            result: serde_json::json!({
                "preview": true,
                "out_dir": out_dir,
                "commands": ["cargo build --release --target wasm32v1-none", "pnpm install --frozen-lockfile", "forge build"]
            }),
            artifact: None,
        });
    }
    if out_dir.exists() {
        return Err(Error::Conflict(format!(
            "artifact output already exists: {}",
            out_dir.display()
        )));
    }
    if lock.layerzero_source.source_archive_sha256.is_none() {
        return Err(Error::Custody(
            "artifact build disabled until the isolated LayerZero source archive digest is frozen"
                .into(),
        ));
    }
    Err(Error::Chain(
        "artifact build requires the qualified external LayerZero source checkout and build tools"
            .into(),
    ))
}

fn verify_hash(path: &Path, expected: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Error::Custody(format!(
            "artifact input must be a regular file: {}",
            path.display()
        )));
    }
    let actual = sha256(&fs::read(path)?);
    if actual != expected {
        return Err(Error::Custody(format!(
            "artifact hash mismatch for {}",
            path.display()
        )));
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
