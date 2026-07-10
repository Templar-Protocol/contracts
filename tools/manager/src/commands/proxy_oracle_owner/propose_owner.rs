use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_owner as spec;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct ProposeOwner {
    /// Proxy-oracle account.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Account to propose as the new owner (omit to clear any pending proposal).
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: Option<AccountId>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl ProposeOwner {
    pub fn into_spec(self) -> spec::ProposeOwner {
        spec::ProposeOwner {
            oracle_id: self.oracle_id,
            account_id: self.account_id,
        }
    }
}
