use clap::Args;
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_oracle_updates_dispatch::PythSourceArgs;
use templar_gateway_oracle_updates_spec::oracle as spec;

use crate::commands::{dedup_price_ids, proxy_oracle::parse_price_identifier, signer::SignerArgs};

#[derive(Args, Debug)]
pub struct UpdatePyth {
    /// Pyth oracle account to update.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Price identifier (32-byte hex) to fetch and update; repeat the flag per feed.
    /// Duplicates are dropped.
    #[arg(long = "price-id", value_name = "HEX", value_parser = parse_price_identifier, required = true)]
    price_ids: Vec<PriceIdentifier>,
    #[command(flatten)]
    pub(crate) sources: PythSourceArgs,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl UpdatePyth {
    pub fn into_spec(self) -> spec::UpdatePyth {
        spec::UpdatePyth {
            oracle_id: self.oracle_id,
            price_ids: dedup_price_ids(self.price_ids),
        }
    }
}
