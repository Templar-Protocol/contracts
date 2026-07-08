use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::registry as spec;
use templar_gateway_types::ContractKind;

use crate::commands::pagination::PaginationArgs;

#[derive(Args, Debug)]
pub struct ListDeploymentsByKind {
    /// Registry to list deployments from.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Contract kind to filter by.
    #[arg(long, value_enum)]
    kind: ContractKind,
    #[command(flatten)]
    pagination: PaginationArgs,
}

impl ListDeploymentsByKind {
    pub fn into_spec(self) -> spec::ListDeploymentsByKind {
        spec::ListDeploymentsByKind {
            registry_id: self.registry_id,
            args: self.pagination.into(),
            kind: self.kind,
        }
    }
}
