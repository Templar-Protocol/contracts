use std::collections::HashMap;

use near_sdk::AccountId;
use near_sdk::serde_json::json;
use templar_common::primitive_types::U256;
use templar_common::oracle::redstone::{FeedData, FeedId};

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

impl FeedGet {
    #[tracing::instrument(skip_all, name = "redstone_adapter_feed_get", fields(adapter_id = %self.adapter_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let feed_ids: Vec<FeedId> = self.feed_id.iter().map(|s| FeedId::from(s.as_str())).collect();

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
            println!("No feed data found");
            return Ok(());
        }

        for (feed_id, feed_data) in &data {
            println!("{feed_id}:");
            println!("  price:             {}", U256::from(feed_data.price));
            println!("  package_timestamp: {}", feed_data.package_timestamp);
            println!("  write_timestamp:   {}", feed_data.write_timestamp);
        }

        Ok(())
    }
}
