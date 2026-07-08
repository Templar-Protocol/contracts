use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_owner as spec;

/// Read the current owner of a proxy-oracle account.
#[derive(Args, Debug)]
pub struct GetOwner {
    /// Proxy-oracle account.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
}

impl GetOwner {
    pub fn into_spec(self) -> spec::GetOwner {
        spec::GetOwner {
            oracle_id: self.oracle_id,
        }
    }
}
