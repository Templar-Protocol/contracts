use std::path::PathBuf;

use clap::Args;
use near_account_id::AccountId;
use templar_common::oracle::redstone::FeedId;
use templar_gateway_methods_spec::redstone as spec;
use templar_gateway_types::Base64Bytes;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct UpdatePrices {
    /// RedStone adapter account to update.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Feed IDs to fetch and update (e.g. BTC, ETH, NEAR).
    #[arg(long = "feed-id", value_name = "FEED_ID", required = true)]
    feed_ids: Vec<FeedId>,
    /// Path to the Node.js binary that runs the RedStone bridge.
    #[arg(
        long,
        env = "REDSTONE_NODE_PATH",
        default_value = "node",
        value_name = "PATH"
    )]
    node_path: PathBuf,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl UpdatePrices {
    pub fn feed_ids(&self) -> &[FeedId] {
        &self.feed_ids
    }

    pub fn node_path(&self) -> &std::path::Path {
        &self.node_path
    }

    /// Build the on-chain write spec from a bridge-fetched payload.
    pub fn into_spec(self, payload: Vec<u8>) -> spec::WritePrices {
        spec::WritePrices {
            oracle_id: self.oracle_id,
            feed_ids: self.feed_ids,
            payload: Base64Bytes(payload),
        }
    }
}
