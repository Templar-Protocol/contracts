use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::registry as spec;

use crate::commands::pagination::PaginationArgs;

#[derive(Args, Debug)]
pub struct ListDeployments {
    /// Registry to list deployments from.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[command(flatten)]
    pagination: PaginationArgs,
}

impl ListDeployments {
    pub fn into_spec(self) -> spec::ListDeployments {
        spec::ListDeployments {
            registry_id: self.registry_id,
            args: self.pagination.into(),
        }
    }
}
