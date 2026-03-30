use std::path::PathBuf;

use anyhow::Context;
use templar_common::oracle::redstone::Config;

use crate::{
    commands::deployment::{Deploy, DeploymentSpec},
    util::ArgsProvider,
    Runner,
};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RedStoneAdapterInitArgs {
    pub config: Config,
}

#[derive(clap::Args, Debug)]
#[group(required = true, multiple = false)]
pub struct ConfigSource {
    /// Use the production RedStone configuration
    #[arg(long)]
    pub prod: bool,
    /// Use the test RedStone configuration
    #[arg(long)]
    pub test: bool,
    /// JSON configuration
    #[arg(long)]
    pub args: Option<String>,
    /// Path to a JSON configuration file
    #[arg(long)]
    pub args_file: Option<PathBuf>,
}

impl ArgsProvider<RedStoneAdapterInitArgs> for ConfigSource {
    fn parse(&self) -> anyhow::Result<RedStoneAdapterInitArgs> {
        if self.prod {
            return Ok(RedStoneAdapterInitArgs {
                config: templar_common::oracle::redstone::config::prod(),
            });
        }
        if self.test {
            return Ok(RedStoneAdapterInitArgs {
                config: templar_common::oracle::redstone::config::test(),
            });
        }
        if let Some(args) = &self.args {
            return serde_json::from_str(args).context("args deserialization");
        }
        if let Some(args_file) = &self.args_file {
            return serde_json::from_reader(std::fs::File::open(args_file)?)
                .context("args deserialization");
        }
        anyhow::bail!("no configuration provided");
    }
}

impl ConfigSource {
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
    type ArgsArgs = ConfigSource;
    type Version = ();

    const PACKAGE_ID: &'static str = "templar-redstone-adapter-contract";
}

impl DeployRedStoneAdapter {
    #[tracing::instrument(skip_all, name = "deploy_redstone_adapter")]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        self.deploy.run(ctx, &()).await
    }
}
