use std::fmt::Debug;

use clap::{Args, ValueEnum};

use crate::{
    util::{ContractLoader, SignerArgs},
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
    // /// Name of the contract to deploy
    // #[arg(value_enum, index = 1)]
    // pub package: Package,
    #[command(flatten)]
    pub args: C::ArgsArgs,
    #[command(flatten)]
    pub signer: SignerArgs,
}

impl<C: DeploymentSpec> Runner<()> for Direct<C> {
    type Output = ();

    async fn run(&self, ctx: &crate::CliContext, _input: &()) -> anyhow::Result<Self::Output> {
        ctx.batch(&self.signer.signer(), &self.signer.account_id)
            .deploy(
                &self
                    .loader
                    .load_contract::<C::Version>(C::PACKAGE_ID)?
                    .wasm_bytes,
            )
            .transact()
            .await
    }
}
