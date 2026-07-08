use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_owner as spec;

/// Renounce ownership of a proxy-oracle account.
#[derive(Args, Debug)]
pub struct RenounceOwner {
    /// Proxy-oracle account.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
}

impl RenounceOwner {
    pub fn into_spec(self) -> spec::RenounceOwner {
        spec::RenounceOwner {
            oracle_id: self.oracle_id,
        }
    }
}
