use clap::Args;
use templar_tools_common::near::Function;

use crate::{
    util::{ContractLoader, LoadArgs, SignerArgs},
    Runner,
};

use super::DeploymentSpec;

#[derive(Args)]
pub struct Direct<C: DeploymentSpec> {
    #[command(flatten)]
    pub loader: ContractLoader,
    #[command(flatten)]
    pub args: C::ArgsLoader,
    #[command(flatten)]
    pub signer: SignerArgs,
}

impl<C: DeploymentSpec> Direct<C> {
    pub fn new(
        loader: crate::util::ContractLoader,
        args: C::ArgsLoader,
        signer: SignerArgs,
    ) -> Self {
        Self {
            loader,
            args,
            signer,
        }
    }
}

impl<C: DeploymentSpec> Runner<()> for Direct<C> {
    type Output = ();

    async fn run(&self, ctx: &crate::CliContext, _input: &()) -> anyhow::Result<Self::Output> {
        let args = self.args.load_vec()?;

        ctx.batch(&self.signer.signer(), &self.signer.account_id)
            .deploy(
                &self
                    .loader
                    .load_artifact::<C::Version>(C::ARTIFACT)?
                    .wasm_bytes,
            )
            .call(Function::new("new").args(args).max_gas())
            .transact()
            .await
    }
}
