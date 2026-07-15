use std::path::PathBuf;

use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::pyth as spec;

use crate::commands::{resolve_base64_arg, signer::SignerArgs};

#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("update_data").args(["data_base64", "data_base64_file"]).required(true)
))]
pub struct UpdatePriceFeeds {
    /// Pyth oracle account to update.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Base64-encoded Pyth update data.
    #[arg(long, value_name = "BASE64")]
    data_base64: Option<String>,
    /// Path to a file containing base64-encoded Pyth update data.
    #[arg(long, value_name = "PATH")]
    data_base64_file: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl UpdatePriceFeeds {
    pub fn try_into_spec(self) -> anyhow::Result<spec::UpdatePriceFeeds> {
        Ok(spec::UpdatePriceFeeds {
            oracle_id: self.oracle_id,
            data: resolve_base64_arg(self.data_base64, self.data_base64_file, "Pyth update data")?,
        })
    }
}
