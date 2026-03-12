use near_sdk::AccountId;
use templar_common::oracle::redstone::Config;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct AdapterConfig {
    /// RedStone adapter contract account ID
    #[arg(long)]
    adapter_id: AccountId,
}

impl AdapterConfig {
    #[tracing::instrument(skip_all, name = "redstone_adapter_config", fields(adapter_id = %self.adapter_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let config: Config = ctx
            .near
            .view(&self.adapter_id, "get_config")
            .await?
            .json()?;

        println!("{}", serde_json::to_string_pretty(&config)?);

        Ok(())
    }
}
