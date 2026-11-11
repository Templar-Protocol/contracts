use std::path::PathBuf;

use clap::{Args, Subcommand};
use near_account_id::AccountId;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum GeneratePatchNs {
    /// Author a guarded configuration replacement without signing or submitting.
    Config(Config),
}

#[derive(Args, Debug)]
pub struct Config {
    /// Deployed market whose finalized configuration and state will be read.
    #[arg(value_name = "MARKET")]
    pub(crate) market_id: AccountId,

    /// RFC 6902 JSON Patch file containing only test/replace operations; - reads stdin.
    #[arg(long, value_name = "PATH")]
    pub(crate) changes: PathBuf,

    /// New schema-3 patch spec; its .blobs sidecar must not already exist either.
    #[arg(long, value_name = "PATH")]
    pub(crate) out: PathBuf,
}
