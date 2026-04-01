use std::path::PathBuf;

use anyhow::Context;
use templar_common::oracle::redstone::Config;

use crate::{
    commands::deployment::{Deploy, DeploymentSpec},
    util::LoadArgs,
    Runner,
};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RedStoneAdapterInitArgs {
    pub config: Config,
}

#[derive(clap::Args, Debug)]
#[group(required = true, multiple = false)]
pub struct RedStoneArgsLoader {
    /// Use the production RedStone configuration
    #[arg(long)]
    prod: bool,
    /// Use the test RedStone configuration
    #[arg(long)]
    test: bool,
    /// JSON initialization arguments
    #[arg(long)]
    args: Option<String>,
    /// Path to a JSON initialization arguments file
    #[arg(long)]
    args_file: Option<PathBuf>,
}

impl LoadArgs<RedStoneAdapterInitArgs> for RedStoneArgsLoader {
    fn load(&self) -> anyhow::Result<RedStoneAdapterInitArgs> {
        match (self.prod, self.test, &self.args, &self.args_file) {
            (true, false, None, None) => Ok(RedStoneAdapterInitArgs {
                config: templar_common::oracle::redstone::config::prod(),
            }),
            (false, true, None, None) => Ok(RedStoneAdapterInitArgs {
                config: templar_common::oracle::redstone::config::test(),
            }),

            (false, false, Some(args), None) => {
                serde_json::from_str(args).context("args deserialization")
            }
            (false, false, None, Some(args_file)) => {
                serde_json::from_reader(std::fs::File::open(args_file)?)
                    .context("args file deserialization")
            }
            _ => anyhow::bail!("Exactly one source must be specified"),
        }
    }
}

impl RedStoneArgsLoader {
    pub fn prod() -> Self {
        Self {
            prod: true,
            test: false,
            args: None,
            args_file: None,
        }
    }

    pub fn test() -> Self {
        Self {
            prod: false,
            test: true,
            args: None,
            args_file: None,
        }
    }

    pub fn from_json_string(args: String) -> Self {
        Self {
            prod: false,
            test: false,
            args: Some(args),
            args_file: None,
        }
    }

    pub fn from_file(args_file: PathBuf) -> Self {
        Self {
            prod: false,
            test: false,
            args: None,
            args_file: Some(args_file),
        }
    }
}

#[derive(clap::Args)]
pub struct DeployRedStoneAdapter {
    #[command(subcommand)]
    pub deploy: Deploy<Self>,
}

impl DeploymentSpec for DeployRedStoneAdapter {
    type Args = RedStoneAdapterInitArgs;
    type ArgsLoader = RedStoneArgsLoader;
    type Version = ();

    const PACKAGE_ID: &'static str = "templar-redstone-adapter-contract";
}

impl DeployRedStoneAdapter {
    #[tracing::instrument(skip_all, name = "deploy_redstone_adapter")]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        self.deploy.run(ctx, &()).await
    }
}
