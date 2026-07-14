use std::path::PathBuf;

use clap::Args;
use near_account_id::AccountId;
use templar_gateway_oracle_updates_spec::oracle as spec;

use crate::commands::{resolve_base64_arg, signer::SignerArgs};

/// Takes the VAA in the request body, so this command reaches no payload source and
/// needs no source flags.
#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("vaa").args(["vaa_base64", "vaa_base64_file"]).required(true)
))]
pub struct UpdatePyth {
    /// Pyth oracle account to update.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Base64-encoded Pyth VAA.
    #[arg(long, value_name = "BASE64")]
    vaa_base64: Option<String>,
    /// Path to a file containing a base64-encoded Pyth VAA.
    #[arg(long, value_name = "PATH")]
    vaa_base64_file: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl UpdatePyth {
    pub fn try_into_spec(self) -> anyhow::Result<spec::UpdatePyth> {
        Ok(spec::UpdatePyth {
            oracle_id: self.oracle_id,
            vaa: resolve_base64_arg(self.vaa_base64, self.vaa_base64_file, "Pyth VAA")?,
        })
    }
}
