use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle as spec;

use crate::commands::pagination::PaginationArgs;
use crate::resolve::OracleTarget;

#[derive(Args, Debug)]
pub struct ListProxies {
    #[command(flatten)]
    pub(crate) target: OracleTarget,
    #[command(flatten)]
    pagination: PaginationArgs,
}

impl ListProxies {
    pub fn into_spec(self, oracle_id: AccountId) -> spec::ListProxies {
        spec::ListProxies {
            oracle_id,
            offset: self.pagination.offset,
            count: self.pagination.count,
        }
    }
}
