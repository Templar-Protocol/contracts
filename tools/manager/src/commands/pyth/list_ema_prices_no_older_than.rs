use clap::Args;
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_methods_spec::pyth as spec;

use crate::commands::proxy_oracle::parse_price_identifier;

#[derive(Args, Debug)]
pub struct ListEmaPricesNoOlderThan {
    /// Pyth oracle account to query.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Price identifier (32-byte hex); repeat the flag per feed.
    #[arg(long = "price-id", value_name = "HEX", value_parser = parse_price_identifier, required = true)]
    price_ids: Vec<PriceIdentifier>,
    /// Reject prices older than this many seconds.
    #[arg(long, value_name = "SECONDS")]
    age_s: u64,
}

impl ListEmaPricesNoOlderThan {
    pub fn into_spec(self) -> spec::ListEmaPricesNoOlderThan {
        spec::ListEmaPricesNoOlderThan {
            oracle_id: self.oracle_id,
            price_ids: self.price_ids,
            age: self.age_s,
        }
    }
}
