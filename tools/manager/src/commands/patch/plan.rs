use std::path::PathBuf;

use clap::Args;
use near_account_id::AccountId;
use near_api::PublicKey;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct Plan {
    /// Path to the patch spec.
    pub(crate) path: PathBuf,
    /// Where to write the plan. Omit to print it.
    #[arg(long, value_name = "PATH")]
    pub(crate) out: Option<PathBuf>,
    /// Account that will sign the atomic batch.
    #[arg(long, env = "SIGNER_ID", value_name = "ACCOUNT_ID")]
    pub(crate) signer_id: AccountId,
    /// Full-access public key used by the signer on the target account.
    #[arg(long, value_name = "PUBLIC_KEY")]
    pub(crate) public_key: PublicKey,
    /// Ignore one named preflight check.
    #[arg(long = "skip-check", value_name = "CHECK_ID")]
    pub(crate) skip_check: Vec<String>,
    /// Permit a set or remove with no in-receipt expectation.
    #[arg(long)]
    pub(crate) allow_unguarded: bool,
}

#[derive(Args, Debug)]
pub struct Apply {
    /// Path to a plan written by `patch plan`.
    #[arg(long, value_name = "PATH")]
    pub(crate) plan: PathBuf,
    /// Re-read every absolute `file` reference at its original path; missing,
    /// moved, or changed bytes abort re-derivation before any transaction is sent.
    #[arg(long = "skip-check", value_name = "CHECK_ID")]
    pub(crate) skip_check: Vec<String>,
    /// Permit a plan that contains an unguarded mutation.
    #[arg(long)]
    pub(crate) allow_unguarded: bool,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}
