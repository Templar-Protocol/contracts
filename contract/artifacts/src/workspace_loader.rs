//! Read WASM bytes at runtime from `target/near/{name}/{name}.wasm`.
//!
//! Enabled via the `workspace-loader` feature.

use crate::ArtifactMetadata;
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

pub(crate) fn target_near_wasm_path_from_meta(
    target_dir: &std::path::Path,
    target_name: &str,
) -> PathBuf {
    target_dir
        .join("near")
        .join(target_name)
        .join(format!("{target_name}.wasm"))
}

fn resolve_target_name(package: &cargo_metadata::Package) -> String {
    crate::find_by_package_name(package.name.as_str()).map_or_else(
        || package.name.replace('-', "_"),
        |artifact| artifact.cargo_target_name.to_owned(),
    )
}

// ---------------------------------------------------------------------------
// Workspace root discovery
// ---------------------------------------------------------------------------

/// Discover the Cargo workspace root from the current directory using
/// `cargo metadata`.
///
/// Returns `None` if `cargo metadata` fails (e.g. not in a Cargo workspace).
#[cfg(feature = "clap")]
pub(crate) fn discover_workspace_root() -> Option<PathBuf> {
    cargo_metadata::MetadataCommand::new()
        .no_deps()
        .exec()
        .ok()
        .map(|meta| meta.workspace_root.as_std_path().to_path_buf())
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
    let metadata = get_metadata(workspace_dir)?;
    if find_package(&metadata, artifact.package_name).is_none() {
        return Err(LoadError::PackageNotFound(
            artifact.package_name.to_string(),
        ));
    }

    let path = target_near_wasm_path_from_meta(
        metadata.target_directory.as_std_path(),
        artifact.cargo_target_name,
    );
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

    let target_name = resolve_target_name(&package);
    let path =
        target_near_wasm_path_from_meta(metadata.target_directory.as_std_path(), &target_name);
    let bytes = std::fs::read(&path).map_err(|source| LoadError::ReadWasm { path, source })?;
    Ok((bytes, package))
}

pub fn build_artifact(
    workspace_dir: &Path,
    package_name: &str,
    reproducible: bool,
) -> Result<Vec<u8>, BuildContractError> {
    let metadata = get_metadata(workspace_dir)?;
    let package = find_package(&metadata, package_name)
        .ok_or_else(|| BuildContractError::PackageNotFound(package_name.to_string()))?;

    let status = build_command(workspace_dir, package.manifest_path.as_str(), reproducible)
        .status()
        .map_err(BuildContractError::Io)?;

    if !status.success() {
        return Err(BuildContractError::BuildStatus { status });
    }

    let target_name = resolve_target_name(package);
    let path =
        target_near_wasm_path_from_meta(metadata.target_directory.as_std_path(), &target_name);
    let bytes =
        std::fs::read(&path).map_err(|source| BuildContractError::ReadWasm { path, source })?;
    Ok(bytes)
}

#[cfg(feature = "clap")]
pub fn spawn_artifact_build(
    workspace_dir: &Path,
    artifact: &ArtifactMetadata,
    reproducible: bool,
) -> std::io::Result<std::process::Child> {
    let manifest_path = artifact.manifest_path();
    let mut command = build_command(workspace_dir, manifest_path, reproducible);
    configure_build_process_group(&mut command);
    command.spawn()
}

#[cfg(all(feature = "clap", unix))]
fn configure_build_process_group(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
}

#[cfg(all(feature = "clap", not(unix)))]
fn configure_build_process_group(_command: &mut std::process::Command) {}

fn build_command(
    workspace_dir: &Path,
    manifest_path: impl AsRef<std::ffi::OsStr>,
    reproducible: bool,
) -> std::process::Command {
    let mut command = std::process::Command::new("cargo");
    let build_mode = if reproducible {
        "reproducible-wasm"
    } else {
        "non-reproducible-wasm"
    };
    command
        .args(["near", "build", build_mode, "--manifest-path"])
        .arg(manifest_path)
        .current_dir(workspace_dir);
    command
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_path_matches_metadata_path() {
        let path = target_near_wasm_path_from_meta(Path::new("/custom-target"), "mock_ft");

        assert_eq!(path, Path::new("/custom-target/near/mock_ft/mock_ft.wasm"));
    }

    #[test]
    fn test_catalog_manifest_path_points_to_cargo_toml() {
        let artifact = crate::ArtifactId::MockFt.metadata();

        assert_eq!(artifact.package_name, "mock-ft");
        let path = Path::new("/ws").join(artifact.manifest_path());

        assert_eq!(path, Path::new("/ws/mock/ft/Cargo.toml"));
    }
}
