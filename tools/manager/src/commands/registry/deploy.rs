use anyhow::Context as _;
use clap::{ArgGroup, Args};
use templar_gateway_methods_spec::registry as spec;

use crate::commands::deploy_common::DeployCommonArgs;
use crate::commands::signer::SignerArgs;

/// Deploy an already-registered contract version to a new account, granting full
/// access keys (the signer's by default) so the operator retains control.
///
/// Init args must be given explicitly — inline `--init-args` or `--init-args-file`,
/// exactly one — so a no-arg-init contract has to state its intent with
/// `--init-args null` rather than falling back to it silently.
#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("init")
        .args(["init_args", "init_args_file"])
        .required(true)
        .multiple(false)
))]
pub struct Deploy {
    #[command(flatten)]
    common: DeployCommonArgs,
    /// Contract init args as an inline JSON string (e.g. `null` or `{"owner_id":"a.near"}`).
    #[arg(long, value_name = "JSON")]
    init_args: Option<String>,
    /// JSON file with the contract's init args.
    #[arg(long, value_name = "PATH")]
    init_args_file: Option<std::path::PathBuf>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Deploy {
    pub fn try_into_spec(self) -> anyhow::Result<spec::Deploy> {
        let signer_public_key = self.signer.public_key()?;
        // clap's required, mutually-exclusive `init` group guarantees exactly one
        // source is present.
        let init_bytes = match (self.init_args, self.init_args_file) {
            (Some(json), _) => {
                serde_json::from_str::<serde_json::Value>(&json)
                    .context("--init-args is not valid JSON")?;
                json.into_bytes()
            }
            (None, Some(path)) => std::fs::read(&path)
                .with_context(|| format!("read init args from {}", path.display()))?,
            (None, None) => unreachable!("clap requires one of --init-args / --init-args-file"),
        };
        Ok(self.common.into_deploy(signer_public_key, init_bytes))
    }
}
