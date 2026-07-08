use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle as spec;

use super::parse_price_identifier;

#[derive(Args, Debug)]
pub struct PriceFeedExists {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long, value_name = "HEX")]
    price_id: String,
}

impl PriceFeedExists {
    pub fn try_into_spec(self) -> anyhow::Result<spec::PriceFeedExists> {
        Ok(spec::PriceFeedExists {
            oracle_id: self.oracle_id,
            price_identifier: parse_price_identifier(&self.price_id)?,
        })
    }
}
