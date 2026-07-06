use std::path::Path;

use anyhow::Context;
use templar_contract_artifacts::{
    find_by_id, find_by_package_name, format_version_key, load_artifact_bytes, ContractArtifact,
};
use templar_gateway_types::Version;

fn get_metadata(workspace_dir: &Path) -> anyhow::Result<cargo_metadata::Metadata> {
    cargo_metadata::MetadataCommand::new()
        .no_deps()
        .current_dir(workspace_dir)
        .exec()
        .with_context(|| format!("run cargo metadata in {}", workspace_dir.display()))
}

fn get_package_from_metadata<'a>(
    metadata: &'a cargo_metadata::Metadata,
    package: &str,
) -> anyhow::Result<&'a cargo_metadata::Package> {
    let package = metadata
        .workspace_packages()
        .into_iter()
        .find(|p| p.name == package)
        .with_context(|| format!("package not found: {package}"))?;
    Ok(package)
}

fn get_contract_wasm_bytes(
    workspace_dir: &Path,
    metadata: &cargo_metadata::Metadata,
    package: &cargo_metadata::Package,
) -> anyhow::Result<Vec<u8>> {
    if let Some(artifact) = find_by_package_name(package.name.as_str()) {
        return load_artifact_bytes(workspace_dir, artifact)
            .with_context(|| format!("read contract WASM for {}", package.name));
    }

    let name_in_path = package.name.replace('-', "_");

    let path = metadata
        .target_directory
        .join("near")
        .join(name_in_path.as_str())
        .join(format!("{name_in_path}.wasm"));

    std::fs::read(&path).with_context(|| format!("read contract WASM from {}", path.as_str()))
}

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
    let metadata = get_metadata(workspace_dir)?;
    let package = get_package_from_metadata(&metadata, cargo_package)?;

    let bytes = get_contract_wasm_bytes(workspace_dir, &metadata, package)?;
    Ok(loaded_contract(package, bytes))
}

pub fn load_contract_artifact<T>(
    workspace_dir: &Path,
    artifact: ContractArtifact,
) -> anyhow::Result<LoadedContract<T>> {
    let artifact_metadata = find_by_id(artifact)?;
    load_contract(workspace_dir, artifact_metadata.package_name)
}

/// Run `cargo near build reproducible-wasm` in `dir`.
///
/// Used by CLI tools that need to build a contract before uploading it.
pub fn build_contract<T>(
    workspace_dir: &Path,
    cargo_package: &str,
) -> anyhow::Result<LoadedContract<T>> {
    let metadata = get_metadata(workspace_dir)?;
    let package = get_package_from_metadata(&metadata, cargo_package)?;

    let status = std::process::Command::new("cargo")
        .args(["near", "build", "reproducible-wasm"])
        .args(["--manifest-path", package.manifest_path.as_str()])
        .current_dir(workspace_dir)
        .status()
        .with_context(|| format!("run cargo near build in {}", workspace_dir.display()))?;

    anyhow::ensure!(
        status.success(),
        "cargo near build failed in {}",
        workspace_dir.display()
    );

    let bytes = get_contract_wasm_bytes(workspace_dir, &metadata, package)?;
    Ok(loaded_contract(package, bytes))
}

pub fn build_contract_artifact<T>(
    workspace_dir: &Path,
    artifact: ContractArtifact,
) -> anyhow::Result<LoadedContract<T>> {
    let artifact_metadata = find_by_id(artifact)?;
    build_contract(workspace_dir, artifact_metadata.package_name)
}
