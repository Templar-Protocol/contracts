use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::registry as spec;

#[derive(Args, Debug)]
pub struct GetDeployment {
    /// Registry to query.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Deployed account to look up.
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
}

impl GetDeployment {
    pub fn into_spec(self) -> spec::GetDeployment {
        spec::GetDeployment {
            registry_id: self.registry_id,
            account_id: self.account_id,
        }
    }
}
