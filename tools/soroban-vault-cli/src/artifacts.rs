use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use sha2::{Digest, Sha256};

use crate::{
    cli::ArtifactName,
    manifest::{ArtifactRecord, Manifest},
    stellar::{CommandExecutor, Stellar},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactSpec {
    pub key: &'static str,
    pub package: &'static str,
    pub wasm_relative_path: &'static str,
    pub build_output_dir: &'static str,
}

impl ArtifactSpec {
    pub fn from_name(name: ArtifactName) -> Self {
        match name {
            ArtifactName::Vault => Self::vault(),
            ArtifactName::Governance => Self::governance(),
            ArtifactName::ShareToken => Self::share_token(),
            ArtifactName::BlendAdapter => Self::blend_adapter(),
            ArtifactName::Proxy4626 => Self::proxy_4626(),
            ArtifactName::CuratorProxy => Self::curator_proxy(),
        }
    }

    pub fn stack_artifacts(include_blend: bool) -> Vec<Self> {
        let mut artifacts = vec![
            Self::vault(),
            Self::governance(),
            Self::share_token(),
            Self::proxy_4626(),
            Self::curator_proxy(),
        ];
        if include_blend {
            artifacts.push(Self::blend_adapter());
        }
        artifacts
    }

    const fn vault() -> Self {
        Self {
            key: "vault",
            package: "templar-soroban-runtime",
            wasm_relative_path:
                "target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm",
            build_output_dir: "target/wasm32-unknown-unknown/release-soroban",
        }
    }

    const fn governance() -> Self {
        Self {
            key: "governance",
            package: "templar-soroban-governance",
            wasm_relative_path:
                "target/wasm32-unknown-unknown/release-soroban/templar_soroban_governance.wasm",
            build_output_dir: "target/wasm32-unknown-unknown/release-soroban",
        }
    }

    const fn share_token() -> Self {
        Self {
            key: "share_token",
            package: "templar-soroban-share-token",
            wasm_relative_path:
                "target/wasm32-unknown-unknown/release-soroban/templar_soroban_share_token.wasm",
            build_output_dir: "target/wasm32-unknown-unknown/release-soroban",
        }
    }

    const fn blend_adapter() -> Self {
        Self {
            key: "blend_adapter",
            package: "templar-soroban-blend-adapter",
            wasm_relative_path:
                "target/wasm32-unknown-unknown/release-soroban/templar_soroban_blend_adapter.wasm",
            build_output_dir: "target/wasm32-unknown-unknown/release-soroban",
        }
    }

    const fn proxy_4626() -> Self {
        Self {
            key: "proxy_4626",
            package: "templar-4626-proxy-soroban",
            wasm_relative_path:
                "target/wasm32-unknown-unknown/release-soroban/templar_4626_proxy_soroban.wasm",
            build_output_dir: "target/wasm32-unknown-unknown/release-soroban",
        }
    }

    const fn curator_proxy() -> Self {
        Self {
            key: "curator_proxy",
            package: "templar-curator-proxy-soroban",
            wasm_relative_path:
                "target/wasm32-unknown-unknown/release-soroban/templar_curator_proxy_soroban.wasm",
            build_output_dir: "target/wasm32-unknown-unknown/release-soroban",
        }
    }

    pub fn wasm_path(&self, workspace: &Path) -> PathBuf {
        workspace.join(self.wasm_relative_path)
    }

    pub fn output_dir(&self, workspace: &Path) -> PathBuf {
        workspace.join(self.build_output_dir)
    }
}

pub fn ensure_uploaded<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &mut Manifest,
    workspace: &Path,
    spec: ArtifactSpec,
    build: bool,
) -> anyhow::Result<String> {
    let wasm_path = spec.wasm_path(workspace);
    if build || !wasm_path.exists() {
        build_artifact(stellar, workspace, spec)?;
    }
    anyhow::ensure!(
        wasm_path.exists(),
        "artifact {} was not found at {}",
        spec.key,
        wasm_path.display()
    );

    let local_hash = sha256_file(&wasm_path)?;
    if let Some(record) = manifest.artifacts.get(spec.key) {
        if record.local_hash == local_hash {
            if let Some(remote_hash) = &record.remote_wasm_hash {
                if stellar.fetch_wasm_hash(remote_hash)? {
                    return Ok(remote_hash.clone());
                }
            }
        }
    }

    let remote_hash = if stellar.fetch_wasm_hash(&local_hash)? {
        local_hash.clone()
    } else {
        stellar.upload(&wasm_path.display().to_string())?
    };

    manifest.artifacts.insert(
        spec.key.to_string(),
        ArtifactRecord {
            package: spec.package.to_string(),
            wasm_path,
            local_hash,
            remote_wasm_hash: Some(remote_hash.clone()),
            verified: true,
        },
    );
    Ok(remote_hash)
}

fn build_artifact<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    workspace: &Path,
    spec: ArtifactSpec,
) -> anyhow::Result<()> {
    let out_dir = spec.output_dir(workspace);
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("create artifact output dir {}", out_dir.display()))?;
    stellar.build_package(
        &workspace.display().to_string(),
        spec.package,
        &out_dir.display().to_string(),
    )?;
    Ok(())
}

pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let digest = Sha256::digest(bytes);
    Ok(hex::encode(digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_paths_match_workspace_layout() {
        let root = Path::new("/workspace");
        assert_eq!(
            ArtifactSpec::from_name(ArtifactName::Vault).wasm_path(root),
            PathBuf::from(
                "/workspace/target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm"
            )
        );
    }

    #[test]
    fn hashes_file_contents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.wasm");
        fs::write(&path, b"abc").expect("write");
        assert_eq!(
            sha256_file(&path).expect("hash"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
