use clap::Args;
use near_account_id::AccountId;
use templar_common::oracle::redstone::FeedId;
use templar_gateway_oracle_updates_dispatch::RedStoneSourceArgs;
use templar_gateway_oracle_updates_spec::oracle as spec;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct UpdateRedStone {
    /// RedStone adapter account to update.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Feed IDs to fetch and update (e.g. BTC, ETH, NEAR); repeat the flag per feed.
    /// All feeds are written in a single `redstone.writePrices` call.
    #[arg(long = "feed-id", value_name = "FEED_ID", required = true)]
    feed_ids: Vec<FeedId>,
    #[command(flatten)]
    pub(crate) sources: RedStoneSourceArgs,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl UpdateRedStone {
    pub fn into_spec(self) -> spec::UpdateRedStone {
        spec::UpdateRedStone {
            oracle_id: self.oracle_id,
            feed_ids: self.feed_ids,
        }
    }
}
