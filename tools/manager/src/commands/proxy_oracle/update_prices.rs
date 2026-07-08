use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle as spec;

use super::parse_price_identifier;

#[derive(Args, Debug)]
pub struct UpdatePrices {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Price identifiers (hex) to refresh; repeat the flag per feed
    #[arg(long = "price-id", value_name = "HEX", required = true)]
    price_ids: Vec<String>,
}

impl UpdatePrices {
    pub fn try_into_spec(self) -> anyhow::Result<spec::UpdatePrices> {
        let price_ids = self
            .price_ids
            .iter()
            .map(|hex| parse_price_identifier(hex))
            .collect::<anyhow::Result<_>>()?;
        Ok(spec::UpdatePrices {
            oracle_id: self.oracle_id,
            price_ids,
        })
    }
}
