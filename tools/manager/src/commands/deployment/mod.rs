mod from_registry;
pub use from_registry::*;
mod direct;
pub use direct::*;

use serde::{de::DeserializeOwned, Serialize};

use crate::{util::LoadArgs, Runner};

pub trait DeploymentSpec {
    type Args: DeserializeOwned + Serialize;
    type ArgsLoader: LoadArgs<Self::Args>;
    type Version;

    const PACKAGE_ID: &'static str;
}

#[derive(clap::Subcommand)]
pub enum Deploy<C: DeploymentSpec> {
    /// Deploy the contract directly onto an account
    Direct(Direct<C>),
    /// Deploy a contract from a registry
    FromRegistry(FromRegistry<C>),
}

impl<C: DeploymentSpec> Runner<()> for Deploy<C> {
    type Output = ();

    async fn run(&self, ctx: &crate::CliContext, _input: &()) -> anyhow::Result<()> {
        match self {
            Self::Direct(standalone) => standalone.run(ctx, &()).await,
            Self::FromRegistry(from_registry) => from_registry.run(ctx, &()).await,
        }
    }
}

impl<C: DeploymentSpec> Deploy<C> {
    pub fn native(
        signer: crate::util::SignerArgs,
        loader: crate::util::ContractLoader,
        args: C::ArgsLoader,
    ) -> Self {
        Self::Direct(Direct::new(loader, args, signer))
    }

    pub fn from_registry(from_registry: FromRegistry<C>) -> Self {
        Self::FromRegistry(from_registry)
    }
}
