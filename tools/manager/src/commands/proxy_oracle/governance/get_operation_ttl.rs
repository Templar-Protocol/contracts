use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

use super::OperationKind;
use crate::resolve::GovernanceTarget;

#[derive(Args, Debug)]
pub struct GetOperationTtl {
    #[command(flatten)]
    pub(crate) target: GovernanceTarget,
    /// Operation kind to read the TTL for.
    #[arg(long, value_enum)]
    kind: OperationKind,
}

impl GetOperationTtl {
    pub fn into_spec(self, governance_id: AccountId) -> spec::GetOperationTtl {
        spec::GetOperationTtl {
            governance_id,
            kind: self.kind,
        }
    }
}
