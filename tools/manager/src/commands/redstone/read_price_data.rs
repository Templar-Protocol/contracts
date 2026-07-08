use clap::Args;
use near_account_id::AccountId;
use templar_common::oracle::redstone::FeedId;
use templar_gateway_methods_spec::redstone as spec;

#[derive(Args, Debug)]
pub struct ReadPriceData {
    /// RedStone adapter account to query.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Feed IDs to read; repeat the flag per feed.
    #[arg(long = "feed-id", value_name = "FEED_ID", required = true)]
    feed_ids: Vec<FeedId>,
}

impl ReadPriceData {
    pub fn into_spec(self) -> spec::ReadPriceData {
        spec::ReadPriceData {
            oracle_id: self.oracle_id,
            feed_ids: self.feed_ids,
        }
    }
}
