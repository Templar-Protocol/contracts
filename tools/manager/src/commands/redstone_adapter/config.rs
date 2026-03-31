use std::io::Write;

use near_sdk::AccountId;
use templar_common::oracle::redstone::Config;

use crate::{
    util::{OutputArgs, OutputStyle},
    CliContext,
};

#[derive(clap::Args, Debug)]
pub struct AdapterConfig {
    /// RedStone adapter contract account ID
    #[arg(long)]
    pub adapter_id: AccountId,
    #[command(flatten)]
    pub output: OutputArgs,
}

impl AdapterConfig {
    #[tracing::instrument(skip_all, name = "redstone_adapter_config", fields(adapter_id = %self.adapter_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let config: Config = ctx
            .near
            .view(&self.adapter_id, "get_config")
            .await?
            .json()?;

        self.output.print(&config)
    }
}

impl OutputStyle for Config {
    fn human(&self, out: &mut dyn Write) -> anyhow::Result<()> {
        writeln!(out, "{}", serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}
