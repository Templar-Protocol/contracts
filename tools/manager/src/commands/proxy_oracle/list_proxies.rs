use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle as spec;

use crate::commands::pagination::PaginationArgs;

#[derive(Args, Debug)]
pub struct ListProxies {
    /// Proxy-oracle account to list price feeds from.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[command(flatten)]
    pagination: PaginationArgs,
}

impl ListProxies {
    pub fn into_spec(self) -> spec::ListProxies {
        spec::ListProxies {
            oracle_id: self.oracle_id,
            offset: self.pagination.offset,
            count: self.pagination.count,
        }
    }
}
