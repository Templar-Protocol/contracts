use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle as spec;

use super::parse_price_identifier;

#[derive(Args, Debug)]
pub struct GetProxy {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long, value_name = "HEX")]
    price_id: String,
}

impl GetProxy {
    pub fn parse(self) -> anyhow::Result<spec::GetProxy> {
        Ok(spec::GetProxy {
            oracle_id: self.oracle_id,
            id: parse_price_identifier(&self.price_id)?,
        })
    }
}
