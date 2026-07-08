use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_owner as spec;

/// Accept a pending ownership transfer of a proxy-oracle account.
#[derive(Args, Debug)]
pub struct AcceptOwner {
    /// Proxy-oracle account.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
}

impl AcceptOwner {
    pub fn into_spec(self) -> spec::AcceptOwner {
        spec::AcceptOwner {
            oracle_id: self.oracle_id,
        }
    }
}
