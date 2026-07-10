use std::path::PathBuf;

use clap::Args;
use near_account_id::AccountId;
use templar_common::oracle::redstone::FeedId;
use templar_gateway_methods_spec::redstone as spec;

use crate::commands::{resolve_base64_arg, signer::SignerArgs};

#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("payload").args(["payload_base64", "payload_base64_file"]).required(true)
))]
pub struct WritePrices {
    /// RedStone adapter account to write to.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Feed IDs the payload carries; repeat the flag per feed.
    #[arg(long = "feed-id", value_name = "FEED_ID", required = true)]
    feed_ids: Vec<FeedId>,
    /// Base64-encoded RedStone payload.
    #[arg(long, value_name = "BASE64")]
    payload_base64: Option<String>,
    /// Path to a file containing a base64-encoded RedStone payload.
    #[arg(long, value_name = "PATH")]
    payload_base64_file: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl WritePrices {
    pub fn try_into_spec(self) -> anyhow::Result<spec::WritePrices> {
        Ok(spec::WritePrices {
            oracle_id: self.oracle_id,
            feed_ids: self.feed_ids,
            payload: resolve_base64_arg(
                self.payload_base64,
                self.payload_base64_file,
                "RedStone payload",
            )?,
        })
    }
}
