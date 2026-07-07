use std::path::Path;

use anyhow::Context;
use templar_contract_artifacts::{build_artifact, format_version_key, load_artifact};
use templar_gateway_types::Version;

fn version<T>(package: &cargo_metadata::Package) -> Version<T> {
    Version::from((
        package.version.major,
        package.version.minor,
        package.version.patch,
    ))
}

pub struct LoadedContract<T> {
    pub name: String,
    pub version: Version<T>,
    pub wasm_bytes: Vec<u8>,
}

impl<T> LoadedContract<T> {
    pub fn version_key(&self) -> String {
        format_version_key(&self.name, &self.version.to_string(), &self.wasm_bytes)
    }
}

fn loaded_contract<T>(package: &cargo_metadata::Package, wasm_bytes: Vec<u8>) -> LoadedContract<T> {
    LoadedContract {
        name: package.name.to_string(),
        wasm_bytes,
        version: version(package),
    }
}

pub fn load_contract<T>(
    workspace_dir: &Path,
    cargo_package: &str,
) -> anyhow::Result<LoadedContract<T>> {
    let (bytes, package) = load_artifact(workspace_dir, cargo_package)
        .with_context(|| format!("load contract {cargo_package}"))?;
    Ok(loaded_contract(&package, bytes))
}

/// Run `cargo near build reproducible-wasm` in `dir`.
///
/// Used by CLI tools that need to build a contract before uploading it.
pub fn build_contract<T>(
    workspace_dir: &Path,
    cargo_package: &str,
) -> anyhow::Result<LoadedContract<T>> {
    let (bytes, package) = build_artifact(workspace_dir, cargo_package, true)
        .with_context(|| format!("build contract {cargo_package}"))?;
    Ok(loaded_contract(&package, bytes))
}
