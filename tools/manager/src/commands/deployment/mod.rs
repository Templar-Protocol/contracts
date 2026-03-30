mod from_registry;
pub use from_registry::*;
mod native;
pub use native::*;

use serde::{de::DeserializeOwned, Serialize};

use crate::{util::ArgsProvider, Runner};

pub trait DeploymentSpec {
    type Args: DeserializeOwned + Serialize;
    type ArgsArgs: ArgsProvider<Self::Args>;
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
