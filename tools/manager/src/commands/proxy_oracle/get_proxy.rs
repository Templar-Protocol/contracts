use clap::Args;
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_methods_spec::proxy_oracle as spec;

use super::parse_price_identifier;

#[derive(Args, Debug)]
pub struct GetProxy {
    /// Proxy-oracle account to query.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Price identifier (32-byte hex, optional `0x` prefix).
    #[arg(long, value_name = "HEX", value_parser = parse_price_identifier)]
    price_id: PriceIdentifier,
}

impl GetProxy {
    pub fn into_spec(self) -> spec::GetProxy {
        spec::GetProxy {
            oracle_id: self.oracle_id,
            id: self.price_id,
        }
    }
}
