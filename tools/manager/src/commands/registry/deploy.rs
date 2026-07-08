use anyhow::Context as _;
use clap::Args;
use templar_gateway_methods_spec::registry as spec;

use crate::commands::deploy_common::DeployCommonArgs;
use crate::context::CliContext;

/// Deploy an already-registered contract version to a new account, granting full
/// access keys (the signer's by default) so the operator retains control.
#[derive(Args, Debug)]
pub struct Deploy {
    #[command(flatten)]
    common: DeployCommonArgs,
    /// JSON file with the contract's init args (defaults to `null`).
    #[arg(long, value_name = "PATH")]
    init_args_file: Option<std::path::PathBuf>,
}

impl Deploy {
    pub fn try_into_spec(self, ctx: &CliContext) -> anyhow::Result<spec::Deploy> {
        let init_bytes = match self.init_args_file {
            Some(path) => std::fs::read(&path)
                .with_context(|| format!("read init args from {}", path.display()))?,
            None => b"null".to_vec(),
        };
        self.common.into_deploy(ctx, init_bytes)
    }
}
