use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::registry as spec;

use crate::commands::pagination::PaginationArgs;

#[derive(Args, Debug)]
pub struct ListVersions {
    /// Registry to list versions from.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[command(flatten)]
    pagination: PaginationArgs,
}

impl ListVersions {
    pub fn into_spec(self) -> spec::ListVersions {
        spec::ListVersions {
            registry_id: self.registry_id,
            args: self.pagination.into_pagination(),
        }
    }
}
