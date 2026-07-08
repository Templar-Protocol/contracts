use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::registry as spec;
use templar_gateway_types::{common::Pagination, ContractKind};

#[derive(Args, Debug)]
pub struct ListDeploymentsByKind {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long, value_enum)]
    kind: ContractKind,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    limit: Option<u32>,
}

impl ListDeploymentsByKind {
    pub fn parse(self) -> spec::ListDeploymentsByKind {
        spec::ListDeploymentsByKind {
            registry_id: self.registry_id,
            args: Pagination {
                offset: self.offset,
                limit: self.limit,
            },
            kind: self.kind,
        }
    }
}
