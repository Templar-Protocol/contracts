use clap::Args;
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_oracle_updates_dispatch::OracleSourceArgs;
use templar_gateway_oracle_updates_spec::oracle as spec;

use crate::commands::{dedup_price_ids, proxy_oracle::parse_price_identifier, signer::SignerArgs};

/// Resolves the oracle's price dependencies at plan time, so it may reach any of the
/// three payload sources and carries all of their flags.
#[derive(Args, Debug)]
pub struct UpdatePrices {
    /// Oracle account whose prices to refresh (proxy, direct, or LST).
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Price identifier (32-byte hex) to refresh; repeat the flag per feed.
    /// Duplicates are dropped.
    #[arg(long = "price-id", value_name = "HEX", value_parser = parse_price_identifier, required = true)]
    price_ids: Vec<PriceIdentifier>,
    #[command(flatten)]
    pub(crate) sources: OracleSourceArgs,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl UpdatePrices {
    pub fn into_spec(self) -> spec::UpdatePrices {
        spec::UpdatePrices {
            oracle_id: self.oracle_id,
            price_ids: dedup_price_ids(self.price_ids),
        }
    }
}
