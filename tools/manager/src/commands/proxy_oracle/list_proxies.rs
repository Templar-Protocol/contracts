use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle as spec;

#[derive(Args, Debug)]
pub struct ListProxies {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    count: Option<u32>,
}

impl ListProxies {
    pub fn into_spec(self) -> spec::ListProxies {
        spec::ListProxies {
            oracle_id: self.oracle_id,
            offset: self.offset,
            count: self.count,
        }
    }
}
