use std::path::PathBuf;

use clap::Args;
use templar_contract_artifacts::ContractArtifact;
use templar_tools_common::build::{
    build_contract, build_contract_artifact, load_contract, load_contract_artifact, LoadedContract,
};

#[derive(Args)]
pub struct ContractLoader {
    /// Skip the build step and use an existing WASM file. Warning: it may be stale!
    #[arg(long)]
    pub no_build: bool,
    /// Path to the workspace root (defaults to current directory)
    #[arg(short, long, env = "WORKSPACE_PATH", default_value = ".")]
    pub workspace_path: PathBuf,
}

impl ContractLoader {
    pub fn load<V>(&self, package_id: &str) -> anyhow::Result<LoadedContract<V>> {
        if self.no_build {
            load_contract(&self.workspace_path, package_id)
        } else {
            build_contract(&self.workspace_path, package_id)
        }
    }

    pub fn load_artifact<V>(
        &self,
        artifact: ContractArtifact,
    ) -> anyhow::Result<LoadedContract<V>> {
        if self.no_build {
            load_contract_artifact(&self.workspace_path, artifact)
        } else {
            build_contract_artifact(&self.workspace_path, artifact)
        }
    }
}
