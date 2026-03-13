use console::style;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::oracle::{
    proxy::{Entry, Proxy, Source},
    OracleRequest,
};

use super::CliPriceIdentifier;
use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct GetProxy {
    #[arg(long)]
    pub oracle_id: AccountId,
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    pub price_id: CliPriceIdentifier,
    /// Output the raw JSON representation of the proxy
    #[arg(long)]
    pub json: bool,
}

impl GetProxy {
    #[tracing::instrument(skip_all, name = "proxy_get", fields(oracle_id = %self.oracle_id, price_id = %self.price_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let price_id = self.price_id.into_inner();

        let proxy: Option<Proxy> = ctx
            .near
            .view(&self.oracle_id, "get_proxy")
            .args_json(json!({ "id": price_id }))
            .await?
            .json()?;

        let Some(proxy) = proxy else {
            println!("Proxy not found for price ID {}", self.price_id);
            return Ok(());
        };

        if self.json {
            println!("{}", serde_json::to_string_pretty(&proxy)?);
            return Ok(());
        }

        println!(
            "{}: {:?}",
            style("Aggregator").bold(),
            proxy.aggregator.method,
        );
        if let Some(max_age) = proxy.aggregator.filter.max_age {
            println!("  {}: {max_age}", style("max_age").dim());
        }
        if let Some(max_clock_drift) = proxy.aggregator.filter.max_clock_drift {
            println!("  {}: {max_clock_drift}", style("max_clock_drift").dim());
        }
        if let Some(min_sources) = proxy.aggregator.filter.min_sources {
            println!("  {}: {min_sources}", style("min_sources").dim());
        }

        println!("\n{} ({}):", style("Entries").bold(), proxy.entries.len());
        for (i, entry) in proxy.entries.iter().enumerate() {
            print_entry(i, entry);
        }

        Ok(())
    }
}

fn print_entry(index: usize, entry: &Entry) {
    println!(
        "  {} {}={}",
        style(format!("[{index}]")).bold(),
        style("weight").dim(),
        entry.weight,
    );
    match &entry.source {
        Source::Request(request) => {
            print_oracle_request("    ", request);
        }
        Source::Transformer(t) => {
            println!("    {}", style("Transformer").cyan());
            print_oracle_request("      ", &t.request);
            println!(
                "      {}: {}.{}",
                style("call").dim(),
                t.call.account_id,
                t.call.method_name,
            );
            println!("      {}: {:?}", style("action").dim(), t.action);
        }
    }
}

fn print_oracle_request(indent: &str, request: &OracleRequest) {
    match request {
        OracleRequest::Pyth(p) => {
            println!("{indent}{}", style("Pyth").cyan());
            println!("{indent}  {}: {}", style("oracle").dim(), p.oracle_id);
            println!("{indent}  {}: {}", style("price_id").dim(), p.price_id);
        }
        OracleRequest::RedStone(p) => {
            println!("{indent}{}", style("RedStone").cyan());
            println!("{indent}  {}: {}", style("oracle").dim(), p.oracle_id);
            println!("{indent}  {}: {}", style("feed_id").dim(), p.price_id);
        }
    }
}
