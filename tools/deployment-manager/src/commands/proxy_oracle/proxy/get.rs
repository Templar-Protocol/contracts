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
    oracle_id: AccountId,
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    price_id: CliPriceIdentifier,
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

        println!("Aggregator: {:?}", proxy.aggregator.sample);
        if let Some(max_age) = proxy.aggregator.filter.max_age {
            println!("  max_age: {max_age}");
        }
        if let Some(max_clock_drift) = proxy.aggregator.filter.max_clock_drift {
            println!("  max_clock_drift: {max_clock_drift}");
        }
        if let Some(min_sources) = proxy.aggregator.filter.min_sources {
            println!("  min_sources: {min_sources}");
        }

        println!("\nEntries ({}):", proxy.entries.len());
        for (i, entry) in proxy.entries.iter().enumerate() {
            print_entry(i, entry);
        }

        Ok(())
    }
}

fn print_entry(index: usize, entry: &Entry) {
    print!("  [{index}] weight={} ", entry.weight);
    match &entry.source {
        Source::Request(request) => match request {
            OracleRequest::Pyth(p) => {
                println!("Pyth oracle={} price_id={}", p.oracle_id, p.price_id);
            }
            OracleRequest::RedStone(p) => {
                println!("RedStone oracle={} feed_id={}", p.oracle_id, p.price_id);
            }
        },
        Source::Transformer(t) => {
            let request_desc = match &t.request {
                OracleRequest::Pyth(p) => {
                    format!("Pyth oracle={} price_id={}", p.oracle_id, p.price_id)
                }
                OracleRequest::RedStone(p) => {
                    format!("RedStone oracle={} feed_id={}", p.oracle_id, p.price_id)
                }
            };
            println!("Transformer {request_desc}");
            println!(
                "      call: {}.{}",
                t.call.account_id, t.call.method_name
            );
            println!("      action: {:?}", t.action);
        }
    }
}
