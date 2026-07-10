//! Price-push orchestration that reaches off-chain before writing on-chain.

use anyhow::Context as _;

use crate::commands::redstone;
use crate::context::CliContext;

/// Fetch a signed RedStone payload via the Node.js bridge, then write it on-chain
/// through the gateway `redstone.writePrices` operation — the single ergonomic
/// price-push command.
pub(super) async fn update_redstone(
    ctx: CliContext,
    args: redstone::UpdatePrices,
) -> anyhow::Result<()> {
    use templar_redstone_bridge::Bridge;
    use tokio::sync::watch;

    let (kill_tx, _kill_rx) = watch::channel(());
    let bridge = Bridge::new(args.node_path(), kill_tx.clone()).context("start RedStone bridge")?;
    tracing::info!(feeds = ?args.feed_ids(), "fetching prices from RedStone bridge");
    let payload = bridge
        .fetch(args.feed_ids().to_vec())
        .await
        .context("fetch RedStone payload")?;
    drop(kill_tx);

    let signer = args.signer.clone();
    ctx.write(signer, args.into_spec(payload)).await
}
