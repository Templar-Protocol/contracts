use std::path::PathBuf;

use clap::Args;
use near_account_id::AccountId;

#[derive(Args, Debug)]
pub struct Export {
    pub(crate) account_id: AccountId,
    #[arg(long, value_name = "PATH")]
    pub(crate) out: PathBuf,
}
