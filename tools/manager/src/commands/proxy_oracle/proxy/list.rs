use std::io::Write;

use console::style;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_tools_common::near;

use crate::{
    util::{OutputArgs, OutputStyle},
    CliContext,
};

#[derive(serde::Serialize)]
struct ProxyListOutput {
    proxies: Vec<PriceIdentifier>,
}

#[derive(clap::Args, Debug)]
pub struct ListProxies {
    #[arg(long)]
    pub oracle_id: AccountId,
    #[command(flatten)]
    pub output: OutputArgs,
}

impl ListProxies {
    #[tracing::instrument(skip_all, name = "proxy_list", fields(oracle_id = %self.oracle_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let proxies: Vec<PriceIdentifier> =
            near::view(&ctx.near, &self.oracle_id, "list_proxies", json!({})).await?;

        let count = proxies.len();

        self.output.print(&ProxyListOutput { proxies })?;

        tracing::info!(count, "Listed proxies");
        Ok(())
    }
}

impl OutputStyle for ProxyListOutput {
    fn fmt_human(&self, out: &mut dyn Write) -> anyhow::Result<()> {
        if self.proxies.is_empty() {
            writeln!(out, "{}", style("No proxies found.").dim())?;
            return Ok(());
        }

        for price_id in &self.proxies {
            writeln!(out, "  {}", style(price_id).bold())?;
        }

        Ok(())
    }
}
