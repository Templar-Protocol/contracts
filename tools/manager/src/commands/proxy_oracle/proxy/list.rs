use console::style;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct ListProxies {
    #[arg(long)]
    pub oracle_id: AccountId,
}

impl ListProxies {
    #[tracing::instrument(skip_all, name = "proxy_list", fields(oracle_id = %self.oracle_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let proxies: Vec<PriceIdentifier> = ctx
            .near
            .view(&self.oracle_id, "list_proxies")
            .args_json(json!({}))
            .await?
            .json()?;

        if proxies.is_empty() {
            println!("{}", style("No proxies found.").dim());
            return Ok(());
        }

        for price_id in &proxies {
            println!("  {}", style(price_id).bold());
        }

        tracing::info!(count = proxies.len(), "Listed proxies");
        Ok(())
    }
}
