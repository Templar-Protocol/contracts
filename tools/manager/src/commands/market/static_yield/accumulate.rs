use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::market as spec;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct Accumulate {
    /// Market to accumulate on.
    #[arg(long, value_name = "ACCOUNT_ID")]
    market_id: AccountId,
    /// Account whose static yield to accumulate. Defaults to the signer.
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: Option<AccountId>,
    /// Maximum number of snapshots to process. Defaults to all of them.
    #[arg(long, value_name = "COUNT")]
    snapshot_limit: Option<u32>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Accumulate {
    pub fn into_spec(self) -> spec::AccumulateStaticYield {
        spec::AccumulateStaticYield {
            market_id: self.market_id,
            account_id: self.account_id,
            snapshot_limit: self.snapshot_limit,
        }
    }
}
