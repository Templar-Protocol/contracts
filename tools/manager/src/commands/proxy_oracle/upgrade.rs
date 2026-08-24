use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Args, ValueEnum};
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle as spec;
use templar_gateway_types::Base64Bytes;

use crate::commands::proxy_oracle::PreflightArgs;
use crate::commands::signer::SignerArgs;
use crate::resolve::OracleTarget;

/// Upgrade a proxy oracle from a local WASM file.
#[derive(Args, Debug)]
pub struct Upgrade {
    #[command(flatten)]
    pub(crate) target: OracleTarget,
    /// Audited WASM file for the target proxy-oracle release.
    #[arg(long, value_name = "PATH")]
    wasm: PathBuf,
    /// Source-state migration required by the target WASM.
    #[arg(long, value_enum)]
    migration: MigrationArg,
    #[command(flatten)]
    pub(crate) preflight: PreflightArgs,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum MigrationArg {
    V0,
}

impl From<MigrationArg> for spec::Migration {
    fn from(value: MigrationArg) -> Self {
        match value {
            MigrationArg::V0 => Self::V0,
        }
    }
}

impl Upgrade {
    pub fn try_into_spec(self, oracle_id: AccountId) -> anyhow::Result<spec::Upgrade> {
        let wasm = std::fs::read(&self.wasm)
            .with_context(|| format!("read WASM from {}", self.wasm.display()))?;
        Ok(spec::Upgrade {
            oracle_id,
            wasm: Base64Bytes(wasm),
            migration: self.migration.into(),
        })
    }
}
