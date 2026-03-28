use crate::commands::deployment::StandardDeploy;

const PROXY_ORACLE_PACKAGE: &str = "templar-proxy-oracle-contract";

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ProxyOracleInitArgs {}

#[derive(clap::Args, Debug)]
pub struct DeployProxyOracle {
    #[command(flatten)]
    pub deploy: StandardDeploy,
}

impl DeployProxyOracle {
    #[tracing::instrument(skip_all, name = "deploy_proxy_oracle")]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        self.deploy
            .run::<ProxyOracleInitArgs>(ctx, PROXY_ORACLE_PACKAGE)
            .await
    }
}
