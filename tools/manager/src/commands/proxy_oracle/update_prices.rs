use clap::Args;
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_methods_spec::proxy_oracle as spec;

use super::parse_price_identifier;
use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct UpdatePrices {
    /// Proxy-oracle account to refresh.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Price identifier (32-byte hex) to refresh; repeat the flag per feed.
    #[arg(long = "price-id", value_name = "HEX", value_parser = parse_price_identifier, required = true)]
    price_ids: Vec<PriceIdentifier>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl UpdatePrices {
    pub fn into_spec(self) -> spec::UpdatePrices {
        spec::UpdatePrices {
            oracle_id: self.oracle_id,
            price_ids: self.price_ids,
        }
    }
}
