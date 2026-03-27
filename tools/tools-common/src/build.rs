use std::path::Path;

use anyhow::Context;
use sha2::Digest;

use crate::version::Version;

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
    metadata: &cargo_metadata::Metadata,
    package: &cargo_metadata::Package,
) -> anyhow::Result<Vec<u8>> {
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
        let hash = sha2::Sha256::digest(&self.wasm_bytes);
        format!("{}@{}#{}", self.name, self.version, hex::encode(hash))
    }
}

pub fn load_contract<T>(
    workspace_dir: &Path,
    cargo_package: &str,
) -> anyhow::Result<LoadedContract<T>> {
    let metadata = get_metadata(workspace_dir)?;
    let package = get_package_from_metadata(&metadata, cargo_package)?;

    let bytes = get_contract_wasm_bytes(&metadata, package)?;
    Ok(LoadedContract {
        name: package.name.to_string(),
        wasm_bytes: bytes,
        version: version(package),
    })
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

    let bytes = get_contract_wasm_bytes(&metadata, package)?;
    Ok(LoadedContract {
        name: package.name.to_string(),
        wasm_bytes: bytes,
        version: version(package),
    })
}
