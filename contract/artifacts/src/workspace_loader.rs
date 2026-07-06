//! Read WASM bytes at runtime from `target/near/{name}/{name}.wasm`.
//!
//! Enabled via the `workspace-loader` feature.

use crate::{target_near_wasm_path, ArtifactMetadata};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors when loading contract artifacts from the workspace build directory.
#[derive(Error, Debug)]
pub enum LoadError {
    /// Failed to read `cargo metadata` for the workspace.
    #[error("Failed to read cargo metadata: {0}")]
    CargoMetadata(#[from] cargo_metadata::Error),

    /// The requested package was not found in the workspace.
    #[error("Package '{0}' not found in workspace")]
    PackageNotFound(String),

    /// Failed to read the compiled WASM file from disk.
    #[error("Failed to read WASM file at {path}: {source}")]
    ReadWasm {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Errors when invoking `cargo near build`.
#[derive(Error, Debug)]
pub enum BuildContractError {
    /// Failed to read `cargo metadata`.
    #[error("Failed to read cargo metadata: {0}")]
    CargoMetadata(#[from] cargo_metadata::Error),

    /// The requested package was not found in the workspace.
    #[error("Package '{0}' not found in workspace")]
    PackageNotFound(String),

    /// The `cargo near build` command failed.
    #[error("cargo near build failed")]
    BuildFailed,

    /// The `cargo near build` command exited with a non-zero status.
    #[error("cargo near build failed with status: {status}")]
    BuildStatus { status: std::process::ExitStatus },

    /// Failed to spawn or wait on the build process.
    #[error("Failed to execute cargo near build: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to read the resulting WASM file.
    #[error("Failed to read built WASM file at {path}: {source}")]
    ReadWasm {
        path: PathBuf,
        source: std::io::Error,
    },
}

// ---------------------------------------------------------------------------
// Cargo metadata helpers
// ---------------------------------------------------------------------------

/// Read workspace-level `cargo metadata`.
pub(crate) fn get_metadata(
    workspace_dir: &Path,
) -> Result<cargo_metadata::Metadata, cargo_metadata::Error> {
    cargo_metadata::MetadataCommand::new()
        .no_deps()
        .current_dir(workspace_dir)
        .exec()
}

/// Find a workspace package by its Cargo package name.
pub(crate) fn find_package<'a>(
    metadata: &'a cargo_metadata::Metadata,
    package: &str,
) -> Option<&'a cargo_metadata::Package> {
    metadata
        .workspace_packages()
        .into_iter()
        .find(|p| p.name == package)
}

/// Build the `target/near/{name}/{name}.wasm` path from target directory and package name.
fn target_near_wasm_path_from_meta(target_dir: &std::path::Path, package_name: &str) -> PathBuf {
    let name_in_path = package_name.replace('-', "_");
    target_dir
        .join("near")
        .join(&name_in_path)
        .join(format!("{name_in_path}.wasm"))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load the WASM bytes for `artifact` from the `target/near` directory.
///
/// The workspace root is the root of the workspace checkout. Returns an
/// error if the WASM file cannot be read.
pub fn load_artifact_bytes(
    workspace_dir: &Path,
    artifact: &ArtifactMetadata,
) -> Result<Vec<u8>, LoadError> {
    let path = target_near_wasm_path(workspace_dir, artifact);
    std::fs::read(&path).map_err(|source| LoadError::ReadWasm { path, source })
}

/// Load the WASM bytes and resolve version info from Cargo metadata.
pub fn load_artifact(
    workspace_dir: &Path,
    package_name: &str,
) -> Result<(Vec<u8>, cargo_metadata::Package), LoadError> {
    let metadata = get_metadata(workspace_dir)?;
    let package = find_package(&metadata, package_name)
        .cloned()
        .ok_or_else(|| LoadError::PackageNotFound(package_name.to_string()))?;

    let path =
        target_near_wasm_path_from_meta(metadata.target_directory.as_std_path(), &package.name);
    let bytes = std::fs::read(&path).map_err(|source| LoadError::ReadWasm { path, source })?;
    Ok((bytes, package))
}

/// Run `cargo near build reproducible-wasm` for a package in the workspace.
///
/// This mirrors the behaviour of `templar-tools-common::build::build_contract`.
pub fn build_artifact(
    workspace_dir: &Path,
    package_name: &str,
) -> Result<Vec<u8>, BuildContractError> {
    let metadata = get_metadata(workspace_dir)?;
    let package = find_package(&metadata, package_name)
        .ok_or_else(|| BuildContractError::PackageNotFound(package_name.to_string()))?;

    let status = std::process::Command::new("cargo")
        .args(["near", "build", "reproducible-wasm"])
        .args(["--manifest-path", package.manifest_path.as_str()])
        .current_dir(workspace_dir)
        .status()
        .map_err(BuildContractError::Io)?;

    if !status.success() {
        return Err(BuildContractError::BuildStatus { status });
    }

    let path =
        target_near_wasm_path_from_meta(metadata.target_directory.as_std_path(), &package.name);
    let bytes =
        std::fs::read(&path).map_err(|source| BuildContractError::ReadWasm { path, source })?;
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_catalog;

    /// Verify that the target path convention lines up with what
    /// `target_near_wasm_path` produces for the same artifact.
    #[test]
    fn test_target_path_matches_metadata_path() {
        // Test that the path-building helpers produce consistent results
        // without requiring actual cargo_metadata.
        let artifact = artifact_catalog()
            .iter()
            .find(|a| a.package_name == "mock-ft")
            .unwrap();
        let path_from_helper = target_near_wasm_path(Path::new("/ws"), artifact);

        assert_eq!(
            path_from_helper,
            Path::new("/ws/target/near/mock_ft/mock_ft.wasm")
        );
    }
}
