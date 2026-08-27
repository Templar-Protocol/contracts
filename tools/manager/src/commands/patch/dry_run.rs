use std::path::PathBuf;

use clap::Args;

#[derive(Args, Debug)]
pub struct DryRun {
    #[arg(long, value_name = "PATH")]
    pub(crate) plan: PathBuf,
    #[arg(long = "skip-check", value_name = "CHECK_ID")]
    pub(crate) skip_check: Vec<String>,
    /// Permit a set or remove with no in-receipt expectation.
    #[arg(long)]
    pub(crate) allow_unguarded: bool,
}
