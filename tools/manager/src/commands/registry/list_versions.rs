use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::registry as spec;
use templar_gateway_types::common::Pagination;

#[derive(Args, Debug)]
pub struct ListVersions {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    limit: Option<u32>,
}

impl ListVersions {
    pub fn into_spec(self) -> spec::ListVersions {
        spec::ListVersions {
            registry_id: self.registry_id,
            args: Pagination {
                offset: self.offset,
                limit: self.limit,
            },
        }
    }
}
