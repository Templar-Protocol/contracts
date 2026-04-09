use std::fmt::Debug;

use clap::{Args, ValueEnum};
use near_fetch::ops::Function;

use crate::{
    util::{ContractLoader, LoadArgs, SignerArgs},
    Runner,
};

use super::DeploymentSpec;

#[derive(ValueEnum, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Package {
    Registry,
    Market,
    ProxyOracle,
    RedStoneAdapter,
}

impl Package {
    pub fn package_id(self) -> &'static str {
        match self {
            Self::Registry => "templar-registry-contract",
            Self::Market => "templar-market-contract",
            Self::ProxyOracle => "templar-proxy-oracle-contract",
            Self::RedStoneAdapter => "templar-redstone-adapter-contract",
        }
    }
}

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

        ctx.batch(&self.signer.signer(), &self.signer.signer_id)
            .deploy(&self.loader.load::<C::Version>(C::PACKAGE_ID)?.wasm_bytes)
            .call(Function::new("new").args(args).max_gas())
            .transact()
            .await
    }
}
