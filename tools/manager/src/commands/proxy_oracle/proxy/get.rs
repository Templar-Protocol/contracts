use std::io::Write;

use console::style;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::oracle::{
    proxy::{Aggregator, Proxy, Source, WeightedSource},
    pyth::PriceIdentifier,
    OracleRequest,
};

use super::CliPriceIdentifier;
use crate::{
    util::{OutputArgs, OutputStyle},
    CliContext,
};

#[derive(clap::Args, Debug)]
pub struct GetProxy {
    #[arg(long)]
    pub oracle_id: AccountId,
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    pub price_id: CliPriceIdentifier,
    #[command(flatten)]
    pub output: OutputArgs,
}

impl GetProxy {
    #[tracing::instrument(skip_all, name = "proxy_get", fields(oracle_id = %self.oracle_id, price_id = %self.price_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let price_id: PriceIdentifier = self.price_id.into();

        let proxy: Option<Proxy> = ctx
            .near
            .view(&self.oracle_id, "get_proxy")
            .args_json(json!({ "id": price_id }))
            .await?
            .json()?;

        self.output.print_optional(proxy.as_ref(), |out| {
            writeln!(out, "Proxy not found for price ID {}", self.price_id)?;
            Ok(())
        })
    }
}

impl OutputStyle for Proxy {
    fn fmt_human(&self, out: &mut dyn Write) -> anyhow::Result<()> {
        let aggregator_name = self.aggregator.name();

        writeln!(out, "{}: {aggregator_name}", style("Aggregator").bold())?;

        if let Some(max_age) = self.freshness_filter.max_age {
            writeln!(out, "  {}: {max_age}", style("max_age").dim())?;
        }

        if let Some(max_clock_drift) = self.freshness_filter.max_clock_drift {
            writeln!(
                out,
                "  {}: {max_clock_drift}",
                style("max_clock_drift").dim()
            )?;
        }

        match &self.aggregator {
            Aggregator::MedianLow(proxy) => {
                writeln!(
                    out,
                    "  {}: {}",
                    style("min_sources").dim(),
                    proxy.min_sources
                )?;
            }
            Aggregator::MedianHigh(proxy) => {
                writeln!(
                    out,
                    "  {}: {}",
                    style("min_sources").dim(),
                    proxy.min_sources
                )?;
            }
            Aggregator::Priority(_) => {}
        }

        writeln!(out)?;

        match &self.aggregator {
            Aggregator::MedianLow(proxy) => {
                writeln!(
                    out,
                    "{} ({}):",
                    style("Entries").bold(),
                    proxy.sources.len()
                )?;
                for (index, entry) in proxy.sources.iter().enumerate() {
                    write_weighted_entry(out, index, entry)?;
                }
            }
            Aggregator::MedianHigh(proxy) => {
                writeln!(
                    out,
                    "{} ({}):",
                    style("Entries").bold(),
                    proxy.sources.len()
                )?;
                for (index, entry) in proxy.sources.iter().enumerate() {
                    write_weighted_entry(out, index, entry)?;
                }
            }
            Aggregator::Priority(proxy) => {
                writeln!(
                    out,
                    "{} ({}):",
                    style("Entries").bold(),
                    proxy.sources.len()
                )?;
                for (index, source) in proxy.sources.iter().enumerate() {
                    writeln!(out, "  {}", style(format!("[{index}]")).bold())?;
                    write_source(out, "    ", source)?;
                }
            }
        }

        Ok(())
    }
}

fn write_weighted_entry(
    out: &mut dyn Write,
    index: usize,
    entry: &WeightedSource,
) -> anyhow::Result<()> {
    writeln!(
        out,
        "  {} {}={}",
        style(format!("[{index}]")).bold(),
        style("weight").dim(),
        entry.weight,
    )?;
    write_source(out, "    ", &entry.source)
}

fn write_source(out: &mut dyn Write, indent: &str, source: &Source) -> anyhow::Result<()> {
    match source {
        Source::Request(request) => write_oracle_request(out, indent, request),
        Source::Transformer(transformer) => {
            writeln!(out, "{indent}{}", style("Transformer").cyan())?;
            write_oracle_request(out, &format!("{indent}  "), &transformer.request)?;
            writeln!(
                out,
                "{indent}  {}: {}.{}",
                style("call").dim(),
                transformer.call.account_id,
                transformer.call.method_name,
            )?;
            writeln!(
                out,
                "{indent}  {}: {:?}",
                style("action").dim(),
                transformer.action,
            )?;
            Ok(())
        }
    }
}

fn write_oracle_request(
    out: &mut dyn Write,
    indent: &str,
    request: &OracleRequest,
) -> anyhow::Result<()> {
    match request {
        OracleRequest::Pyth(p) => {
            writeln!(out, "{indent}{}", style("Pyth").cyan())?;
            writeln!(out, "{indent}  {}: {}", style("oracle").dim(), p.oracle_id)?;
            writeln!(out, "{indent}  {}: {}", style("price_id").dim(), p.price_id)?;
        }
        OracleRequest::RedStone(p) => {
            writeln!(out, "{indent}{}", style("RedStone").cyan())?;
            writeln!(out, "{indent}  {}: {}", style("oracle").dim(), p.oracle_id)?;
            writeln!(out, "{indent}  {}: {}", style("feed_id").dim(), p.price_id)?;
        }
    }

    Ok(())
}
