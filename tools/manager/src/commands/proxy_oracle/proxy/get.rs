use std::io::Write;

use console::style;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::oracle::{
    proxy::{
        aggregator::source::{Source, WeightedSource},
        Proxy,
    },
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
        let (aggregator_name, max_age, max_clock_drift, min_sources, entries_len) =
            proxy_summary(self);
        writeln!(out, "{}: {:?}", style("Aggregator").bold(), aggregator_name)?;
        if let Some(max_age) = max_age {
            writeln!(out, "  {}: {max_age}", style("max_age").dim())?;
        }
        if let Some(max_clock_drift) = max_clock_drift {
            writeln!(
                out,
                "  {}: {max_clock_drift}",
                style("max_clock_drift").dim()
            )?;
        }
        if let Some(min_sources) = min_sources {
            writeln!(out, "  {}: {min_sources}", style("min_sources").dim())?;
        }

        writeln!(out)?;
        writeln!(out, "{} ({}):", style("Entries").bold(), entries_len)?;
        for (index, entry) in proxy_entries(self).iter().enumerate() {
            write_entry(out, index, entry)?;
        }

        Ok(())
    }
}

fn proxy_summary(
    proxy: &Proxy,
) -> (
    &'static str,
    Option<templar_common::time::Nanoseconds>,
    Option<templar_common::time::Nanoseconds>,
    Option<u32>,
    usize,
) {
    match proxy {
        Proxy::MedianLow(proxy) => (
            "MedianLow",
            proxy.filter.price.max_age,
            proxy.filter.price.max_clock_drift,
            proxy.filter.min_sources,
            proxy.sources.len(),
        ),
        Proxy::MedianHigh(proxy) => (
            "MedianHigh",
            proxy.filter.price.max_age,
            proxy.filter.price.max_clock_drift,
            proxy.filter.min_sources,
            proxy.sources.len(),
        ),
        Proxy::Priority(proxy) => (
            "Priority",
            proxy.filter.price.max_age,
            proxy.filter.price.max_clock_drift,
            proxy.filter.min_sources,
            proxy.sources.len(),
        ),
    }
}

fn proxy_entries(proxy: &Proxy) -> Vec<WeightedSource> {
    match proxy {
        Proxy::MedianLow(proxy) => proxy.sources.clone(),
        Proxy::MedianHigh(proxy) => proxy.sources.clone(),
        Proxy::Priority(proxy) => proxy
            .sources
            .iter()
            .cloned()
            .map(|source| WeightedSource { source, weight: 1 })
            .collect(),
    }
}

fn write_entry(out: &mut dyn Write, index: usize, entry: &WeightedSource) -> anyhow::Result<()> {
    writeln!(
        out,
        "  {} {}={}",
        style(format!("[{index}]")).bold(),
        style("weight").dim(),
        entry.weight,
    )?;
    match &entry.source {
        Source::Request(request) => {
            write_oracle_request(out, "    ", request)?;
        }
        Source::Transformer(t) => {
            writeln!(out, "    {}", style("Transformer").cyan())?;
            write_oracle_request(out, "      ", &t.request)?;
            writeln!(
                out,
                "      {}: {}.{}",
                style("call").dim(),
                t.call.account_id,
                t.call.method_name,
            )?;
            writeln!(out, "      {}: {:?}", style("action").dim(), t.action)?;
        }
    }

    Ok(())
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
