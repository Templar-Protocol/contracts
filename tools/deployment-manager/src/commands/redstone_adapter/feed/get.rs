use std::collections::HashMap;

use console::style;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::oracle::redstone::{FeedData, FeedId, DECIMALS};
use templar_common::primitive_types::U256;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct FeedGet {
    /// RedStone adapter contract account ID
    #[arg(long)]
    adapter_id: AccountId,
    /// Feed IDs to query (e.g. BTC, ETH, NEAR)
    #[arg(long, required = true)]
    feed_id: Vec<String>,
    /// Output raw JSON
    #[arg(long)]
    json: bool,
}

fn format_price(price: U256) -> String {
    #[allow(clippy::expect_used)]
    let divisor = U256::from(10u64.pow(u32::try_from(DECIMALS).expect("DECIMALS fits in u32")));
    let whole = price / divisor;
    let frac = price % divisor;
    // Pad fractional part with leading zeros up to DECIMALS digits, then trim trailing zeros.
    let frac_str = format!("{frac:0>width$}", width = DECIMALS as usize);
    let frac_trimmed = frac_str.trim_end_matches('0');
    if frac_trimmed.is_empty() {
        format!("{whole}")
    } else {
        format!("{whole}.{frac_trimmed}")
    }
}

fn format_timestamp_ms(ms: u64) -> String {
    #[allow(clippy::cast_possible_wrap)]
    chrono::DateTime::from_timestamp_millis(ms as i64).map_or_else(
        || format!("{ms}ms"),
        |dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
}

impl FeedGet {
    #[tracing::instrument(skip_all, name = "redstone_adapter_feed_get", fields(adapter_id = %self.adapter_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let feed_ids: Vec<FeedId> = self
            .feed_id
            .iter()
            .map(|s| FeedId::from(s.as_str()))
            .collect();

        let data: HashMap<FeedId, FeedData> = ctx
            .near
            .view(&self.adapter_id, "read_price_data")
            .args_json(json!({ "feed_ids": feed_ids }))
            .await?
            .json()?;

        if self.json {
            println!("{}", serde_json::to_string_pretty(&data)?);
            return Ok(());
        }

        if data.is_empty() {
            println!("{}", style("No feed data found").dim());
            return Ok(());
        }

        for (feed_id, feed_data) in &data {
            let price = U256::from(feed_data.price);
            println!("{}:", style(feed_id).bold());
            println!(
                "  {}: {}",
                style("price").dim(),
                style(format_price(price)).green(),
            );
            println!(
                "  {}: {}",
                style("package_timestamp").dim(),
                format_timestamp_ms(feed_data.package_timestamp.as_ms())
            );
            println!(
                "  {}: {}",
                style("write_timestamp").dim(),
                format_timestamp_ms(feed_data.write_timestamp.as_ms())
            );
        }

        Ok(())
    }
}
