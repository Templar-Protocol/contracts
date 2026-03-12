use std::path::PathBuf;

use near_fetch::ops::Function;
use near_sdk::{json_types::Base64VecU8, serde_json::json, AccountId, NearToken};
use templar_common::oracle::redstone::FeedId;
use templar_redstone_bridge::Bridge;
use tokio::sync::watch;

use crate::{commands::SignerArgs, CliContext};

#[derive(clap::Args, Debug)]
pub struct UpdatePrices {
    #[command(flatten)]
    signer: SignerArgs,
    /// RedStone adapter contract account ID
    #[arg(long)]
    adapter_id: AccountId,
    /// Feed IDs to fetch and update (e.g. BTC, ETH, NEAR)
    #[arg(long, required = true)]
    feed_id: Vec<FeedId>,
    /// Path to Node.js binary
    #[arg(long, env = "REDSTONE_NODE_PATH", default_value = "node")]
    node_path: PathBuf,
    /// Path to the compiled RedStone bridge JS entry point
    #[arg(long, env = "REDSTONE_BRIDGE_PATH")]
    bridge_path: PathBuf,
}

impl UpdatePrices {
    #[tracing::instrument(skip_all, name = "redstone_adapter_update_prices", fields(adapter_id = %self.adapter_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let (kill_tx, _kill_rx) = watch::channel(());
        let bridge = Bridge::new(&self.node_path, &self.bridge_path, kill_tx.clone());

        tracing::info!(feeds = ?self.feed_id, "Fetching prices from RedStone bridge");
        let payload_bytes = bridge.fetch(self.feed_id.clone()).await?;

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

        drop(kill_tx);
        tracing::info!("Prices updated");
        Ok(())
    }
}
