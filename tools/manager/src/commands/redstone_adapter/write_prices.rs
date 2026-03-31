use std::path::PathBuf;

use anyhow::Context;
use base64::prelude::*;
use near_fetch::ops::Function;
use near_sdk::{json_types::Base64VecU8, serde_json::json, AccountId, NearToken};
use templar_common::oracle::redstone::FeedId;

use crate::util::{load_text, SignerArgs};
use crate::CliContext;

#[derive(clap::Args, Debug)]
#[group(required = true, multiple = false, args = ["payload_base64", "payload_base64_file"])]
pub struct WritePrices {
    /// Signer for the write transaction.
    #[command(flatten)]
    pub signer: SignerArgs,
    /// RedStone adapter contract account ID
    #[arg(long)]
    pub adapter_id: AccountId,
    /// Feed IDs to update (e.g. BTC, ETH, NEAR)
    #[arg(long, required = true)]
    pub feed_id: Vec<FeedId>,
    /// Base64-encoded RedStone payload
    #[arg(long)]
    pub payload_base64: Option<String>,
    /// Path to a file containing a base64-encoded RedStone payload
    #[arg(long)]
    pub payload_base64_file: Option<PathBuf>,
}

impl WritePrices {
    #[tracing::instrument(skip_all, name = "redstone_adapter_write_prices", fields(adapter_id = %self.adapter_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let payload_base64 = load_text(
            self.payload_base64.as_deref(),
            self.payload_base64_file.as_deref(),
            "payload-base64",
        )?;
        let payload_bytes = BASE64_STANDARD
            .decode(payload_base64.trim())
            .context("invalid base64 payload")?;

        tracing::info!(feeds = ?self.feed_id, "Writing prices");

        let signer = self.signer.signer();
        ctx.batch(&signer, &self.adapter_id)
            .call(
                Function::new("write_prices")
                    .args_json(json!({
                        "feed_ids": self.feed_id,
                        "payload": Base64VecU8(payload_bytes),
                    }))
                    .deposit(NearToken::from_yoctonear(0))
                    .max_gas(),
            )
            .transact()
            .await?;

        tracing::info!("Prices written");
        Ok(())
    }
}
