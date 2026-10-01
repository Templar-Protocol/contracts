use clap::Args;
use near_account_id::AccountId;

use crate::commands::signer::SignerArgs;

/// Harvest the signer's static yield from every listed market.
///
/// Each market is processed independently: a failure on one is reported and the
/// rest still run. Exits non-zero if any market or forwarding transfer failed.
#[derive(Args, Debug)]
#[command(group(clap::ArgGroup::new("markets").args(["registry_id", "market_id"]).required(true).multiple(true)))]
pub struct Harvest {
    /// Registries whose every deployment is harvested.
    #[arg(long, value_name = "ACCOUNT_ID", value_delimiter = ',')]
    pub(crate) registry_id: Vec<AccountId>,
    /// Markets to harvest.
    #[arg(long, value_name = "ACCOUNT_ID", value_delimiter = ',')]
    pub(crate) market_id: Vec<AccountId>,
    /// Account to forward the harvested yield to, e.g. a treasury.
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub(crate) receiver_id: Option<AccountId>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}
