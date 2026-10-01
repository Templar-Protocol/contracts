use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::market as spec;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct Withdraw {
    /// Market to withdraw from.
    #[arg(long, value_name = "ACCOUNT_ID")]
    market_id: AccountId,
    /// Amount to withdraw (in the borrow asset's smallest unit). Defaults to
    /// everything accumulated.
    #[arg(long, value_name = "AMOUNT")]
    amount: Option<u128>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Withdraw {
    pub fn into_spec(self) -> spec::WithdrawStaticYield {
        spec::WithdrawStaticYield {
            market_id: self.market_id,
            amount: self.amount.map(Into::into),
        }
    }
}
