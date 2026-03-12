use crate::commands::{DeployFromRegistry, SignerArgs};

#[derive(clap::Args, Debug)]
pub struct CreateProxyOracle {
    #[command(flatten)]
    signer: SignerArgs,

    #[command(flatten)]
    deploy: DeployFromRegistry,
}

impl CreateProxyOracle {
    #[tracing::instrument(skip_all, name = "proxy_oracle_create")]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        tracing::info!("Creating proxy oracle from registry");
        self.deploy
            .run(ctx, &self.signer.signer(), b"{}".to_vec())
            .await?;
        tracing::info!("Proxy oracle created");

        Ok(())
    }
}
