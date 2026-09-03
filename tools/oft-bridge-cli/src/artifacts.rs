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
    pub foundry_toml_sha256: String,
    pub remappings_sha256: String,
    /// Digest of the operator-supplied build dependency archive: the exact
    /// npm package closure (with registry integrity pins) the build extracts.
    pub build_deps_archive_sha256: String,
    pub creation_bytecode_keccak256: String,
    pub runtime_bytecode_keccak256: String,
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

/// Digest of the embedded artifact lock; binds plans to artifact custody.
pub fn lock_sha256() -> Result<String> {
    Ok(sha256(LOCK_BYTES))
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
    verify_hash(
        &root.join("evm/foundry.toml"),
        &lock.evm.foundry_toml_sha256,
    )?;
    verify_hash(
        &root.join("evm/remappings.txt"),
        &lock.evm.remappings_sha256,
    )?;
    Ok(CommandData {
        result: serde_json::json!({"verified": true, "artifact_lock_sha256": sha256(LOCK_BYTES)}),
        artifact: None,
    })
}

pub fn build_command(
    state: &Path,
    out_dir: &Path,
    write: bool,
    deps_archive: Option<&Path>,
) -> Result<CommandData> {
    let lock = embedded_lock()?;
    build_with_executor(
        state,
        out_dir,
        write,
        deps_archive,
        &lock,
        &crate::process::RealExecutor,
    )
}

/// Deterministic EVM artifact build: local preparation, not a chain mutation.
/// The operator-supplied dependency archive is digest-verified against the
/// embedded lock before extraction; `forge` is the only external tool.
pub fn build_with_executor(
    state: &Path,
    out_dir: &Path,
    write: bool,
    deps_archive: Option<&Path>,
    lock: &ArtifactLockV1,
    executor: &dyn crate::process::CommandExecutor,
) -> Result<CommandData> {
    let _route = RouteStore::open(state)?.load_state()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wrapper = root.join("evm/src/DisposableOFT.sol");
    let package = root.join("evm/package.json");
    let foundry_toml = root.join("evm/foundry.toml");
    let remappings = root.join("evm/remappings.txt");
    if !write {
        return Ok(CommandData {
            result: serde_json::json!({
                "preview": true,
                "out_dir": out_dir,
                "commands": [
                    "verify wrapper/package/foundry/remappings/deps-archive digests",
                    "tar -xf <deps-archive> -C <out_dir>/work",
                    "forge build --root <out_dir>/work",
                ],
                "requires_deps_archive": true,
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
    verify_hash(&wrapper, &lock.evm.wrapper_source_sha256)?;
    verify_hash(&package, &lock.evm.package_json_sha256)?;
    verify_hash(&foundry_toml, &lock.evm.foundry_toml_sha256)?;
    verify_hash(&remappings, &lock.evm.remappings_sha256)?;
    let deps = deps_archive.ok_or_else(|| {
        Error::InvalidInput(
            "artifact build --write requires --deps-archive matching the embedded lock".into(),
        )
    })?;
    verify_hash(deps, &lock.evm.build_deps_archive_sha256)?;
    let work = prepare_work_dir(
        out_dir,
        deps,
        [&wrapper, &package, &foundry_toml, &remappings],
        executor,
    )?;
    run_forge(&work, executor)?;
    finalize_build(state, out_dir, &work, lock)
}

/// Runs the pinned Foundry build inside the prepared work dir.
fn run_forge(work: &Path, executor: &dyn crate::process::CommandExecutor) -> Result<()> {
    let work_str = work
        .to_str()
        .ok_or_else(|| Error::InvalidInput("artifact work path must be valid UTF-8".into()))?;
    executor
        .run(
            "forge",
            &[
                "build".to_string(),
                "--root".to_string(),
                work_str.to_string(),
            ],
            &[],
            &[],
        )
        .map_err(|e| Error::Chain(format!("forge build failed: {e}")))?;
    Ok(())
}

/// Reads the forge artifact, verifies the frozen bytecode digests, and
/// persists the chained build report.
fn finalize_build(
    state: &Path,
    out_dir: &Path,
    work: &Path,
    lock: &ArtifactLockV1,
) -> Result<CommandData> {
    let artifact_json: serde_json::Value = serde_json::from_slice(&fs::read(
        work.join("out/DisposableOFT.sol/DisposableOFT.json"),
    )?)?;
    let bytecode_hex = artifact_json
        .get("bytecode")
        .and_then(|b| b.get("object"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let runtime_hex = artifact_json
        .get("deployedBytecode")
        .and_then(|b| b.get("object"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if bytecode_hex.len() < 4 || runtime_hex.len() < 4 {
        return Err(Error::Custody(
            "forge artifact produced empty bytecode".into(),
        ));
    }
    let creation_bytes = hex::decode(bytecode_hex.strip_prefix("0x").unwrap_or(bytecode_hex))
        .map_err(|_| Error::Custody("forge creation bytecode is not hex".into()))?;
    let runtime_bytes = hex::decode(runtime_hex.strip_prefix("0x").unwrap_or(runtime_hex))
        .map_err(|_| Error::Custody("forge runtime bytecode is not hex".into()))?;
    let creation = hex::encode(crate::evm::keccak256_of(&creation_bytes));
    let runtime = hex::encode(crate::evm::keccak256_of(&runtime_bytes));
    // The frozen lock is authoritative: a rebuilt artifact that diverges is
    // a custody failure, never a silent re-freeze.
    for (frozen, actual, label) in [
        (
            &lock.evm.creation_bytecode_keccak256,
            creation.as_str(),
            "creation",
        ),
        (
            &lock.evm.runtime_bytecode_keccak256,
            runtime.as_str(),
            "runtime",
        ),
    ] {
        if frozen != actual {
            return Err(Error::Custody(format!(
                "built {label} bytecode diverges from the frozen artifact lock"
            )));
        }
    }
    let report = serde_json::json!({
        "schema": "artifact_build_report",
        "schema_version": crate::domain::SCHEMA_VERSION,
        "artifact_lock_sha256": sha256(LOCK_BYTES),
        "deps_archive_sha256": lock.evm.build_deps_archive_sha256,
        "evm": {
            "creation_bytecode_keccak256": creation,
            "runtime_bytecode_keccak256": runtime,
        },
        "work_dir": work,
    });
    let report_path = out_dir.join("build-report.json");
    crate::state::write_create_new_json(&report_path, &report)?;
    let lock_sha = sha256(LOCK_BYTES);
    RouteStore::open(state)?.append_operation(
        crate::state::OperationEventV1 {
            operation_id: format!("artifact-build-{}", &lock_sha[..16]),
            state: crate::state::OperationState::Planned,
            detail: serde_json::to_value(crate::domain::LocalPreparationV1::BuildArtifact {
                artifact_lock_sha256: lock_sha,
            })?,
        },
        None,
    )?;
    let artifact = crate::domain::ArtifactRefV1 {
        kind: "artifact_build_report".into(),
        path: report_path,
        sha256: crate::canonical_sha256(&report)?,
        schema_version: crate::domain::SCHEMA_VERSION,
        authoritative: true,
    };
    Ok(CommandData {
        result: report,
        artifact: Some(artifact),
    })
}
/// Extracts the digest-verified dependency archive and materializes the build
/// root from the in-crate inputs.
fn prepare_work_dir(
    out_dir: &Path,
    deps: &Path,
    inputs: [&std::path::PathBuf; 4],
    executor: &dyn crate::process::CommandExecutor,
) -> Result<std::path::PathBuf> {
    let [wrapper, package, foundry_toml, remappings] = inputs;
    let work = out_dir.join("work");
    let node_modules = work.join("node_modules");
    fs::create_dir_all(&node_modules)?;
    let deps_str = deps
        .to_str()
        .ok_or_else(|| Error::InvalidInput("deps archive path must be valid UTF-8".into()))?;
    let node_modules_str = node_modules
        .to_str()
        .ok_or_else(|| Error::InvalidInput("artifact work path must be valid UTF-8".into()))?;
    executor
        .run(
            "tar",
            &[
                "-xf".to_string(),
                deps_str.to_string(),
                "-C".to_string(),
                node_modules_str.to_string(),
            ],
            &[],
            &[],
        )
        .map_err(|e| Error::Chain(format!("deps archive extraction failed: {e}")))?;
    // Materialize the build root: wrapper, manifest, and compiler config.
    fs::create_dir_all(work.join("src"))?;
    fs::copy(wrapper, work.join("src/DisposableOFT.sol"))?;
    fs::copy(package, work.join("package.json"))?;
    fs::copy(foundry_toml, work.join("foundry.toml"))?;
    fs::copy(remappings, work.join("remappings.txt"))?;
    Ok(work)
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
