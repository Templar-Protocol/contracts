use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

use super::OperationKindArg;

#[derive(Args, Debug)]
pub struct GetOperationTtl {
    /// Governance contract account.
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    /// Operation kind to read the TTL for.
    #[arg(long, value_enum)]
    kind: OperationKindArg,
}

impl GetOperationTtl {
    pub fn into_spec(self) -> spec::GetOperationTtl {
        spec::GetOperationTtl {
            governance_id: self.governance_id,
            kind: self.kind.into(),
        }
    }
}
