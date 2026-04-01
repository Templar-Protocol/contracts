use crate::{
    commands::deployment::{Deploy, DeploymentSpec},
    util::EmptyArgsLoader,
    Runner,
};

#[derive(clap::Args)]
pub struct DeployProxyOracle {
    #[command(subcommand)]
    pub deploy: Deploy<Self>,
}

impl DeploymentSpec for DeployProxyOracle {
    type Args = ();
    type ArgsLoader = EmptyArgsLoader;
    type Version = ();

    const PACKAGE_ID: &'static str = "templar-proxy-oracle-contract";
}

impl DeployProxyOracle {
    #[tracing::instrument(skip_all, name = "deploy_proxy_oracle")]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        self.deploy.run(ctx, &()).await
    }
}
