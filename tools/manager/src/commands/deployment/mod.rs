mod from_registry;
pub use from_registry::*;
mod native;
pub use native::*;
use serde::{de::DeserializeOwned, Serialize};

use super::{json_input::ArgsSource, SignerArgs};

#[derive(clap::Subcommand, Debug)]
pub enum Channel {
    /// Deploy the contract directly onto an account
    Native(Native),
    /// Deploy a contract from a registry
    FromRegistry(FromRegistry),
}

impl Channel {
    pub async fn run(
        &self,
        ctx: &crate::CliContext,
        signer_args: &SignerArgs,
        default_package: &str,
        init_args: Vec<u8>,
    ) -> anyhow::Result<()> {
        match self {
            Self::Native(standalone) => {
                standalone
                    .run(ctx, signer_args, init_args, default_package)
                    .await
            }
            Self::FromRegistry(from_registry) => {
                from_registry.run(ctx, signer_args, init_args).await
            }
        }
    }
}

#[derive(clap::Args, Debug)]
pub struct StandardDeploy {
    #[command(flatten)]
    pub signer: SignerArgs,

    #[command(subcommand)]
    pub channel: Channel,

    #[command(flatten)]
    pub args: ArgsSource,
}

impl StandardDeploy {
    pub fn native(
        signer: SignerArgs,
        contract_wasm: super::ContractWasm,
        args: ArgsSource,
    ) -> Self {
        Self {
            signer,
            channel: Channel::Native(Native::new(contract_wasm)),
            args,
        }
    }

    pub fn from_registry(
        signer: SignerArgs,
        from_registry: FromRegistry,
        args: ArgsSource,
    ) -> Self {
        Self {
            signer,
            channel: Channel::FromRegistry(from_registry),
            args,
        }
    }

    #[tracing::instrument(skip_all, name = "deploy", fields(account_id = %self.signer.account_id))]
    pub async fn run<InitArgs: Serialize + DeserializeOwned>(
        &self,
        ctx: &crate::CliContext,
        default_package: &str,
    ) -> anyhow::Result<()> {
        self.channel
            .run(
                ctx,
                &self.signer,
                default_package,
                self.args.load_vec::<InitArgs>()?,
            )
            .await
    }
}
