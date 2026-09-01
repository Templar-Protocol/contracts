use std::path::PathBuf;

use crate::{commands::signer::SignerArgs, spec::plan::DeploymentStage};
use clap::Args;
use near_account_id::AccountId;
use near_api::PublicKey as CliPublicKey;

/// Deliberately takes no credential: planning reads the chain and writes a file,
/// so a mistyped subcommand cannot spend NEAR. The account and public key are
/// still needed — the account signs each planned transaction and the key is
/// granted full access on the accounts created — but neither is a secret, which
/// is why this is not [`SignerArgs`].
#[derive(Args, Debug)]
pub struct Plan {
    /// Path to the market spec.
    pub(crate) path: PathBuf,

    /// Where to write the plan. Omit to print it.
    #[arg(long, value_name = "PATH")]
    pub(crate) out: Option<PathBuf>,

    /// Account that will sign every planned transaction.
    #[arg(long, env = "SIGNER_ID", value_name = "ACCOUNT_ID")]
    pub(crate) signer_id: AccountId,

    /// Public key granted full access on each account the deploy creates.
    #[arg(long, value_name = "PUBLIC_KEY")]
    pub(crate) public_key: CliPublicKey,

    /// Stop after this complete dependency-ordered deployment stage. Each plan
    /// is a fresh cumulative prefix; do not apply separate stage plans in
    /// sequence. Resume an interrupted deployment with the same plan file.
    #[arg(long, value_enum, default_value_t = DeploymentStage::Market)]
    pub(crate) stop_after: DeploymentStage,

    /// Ignore a named check. Every other check still runs, and every derived
    /// value is still derived — this suppresses one verdict, not the preflight.
    ///
    /// For a check that is wrong or over-strict. Reach for editing the plan only
    /// when the spec genuinely cannot express what the market needs.
    #[arg(long = "skip-check", value_name = "CHECK_ID")]
    pub(crate) skip_check: Vec<String>,

    /// Accept a `decimals` override that disagrees with the token's metadata.
    #[arg(long)]
    pub(crate) accept_decimals_mismatch: bool,

    /// Bypass embedded-ABI validation for every registry deployment in this plan.
    #[arg(long)]
    pub(crate) skip_abi_check: bool,
}

/// Send a plan file.
#[derive(Args, Debug)]
pub struct Apply {
    /// Path to a plan written by `market plan`.
    #[arg(long, value_name = "PATH")]
    pub(crate) plan: PathBuf,

    /// Skip the confirmation prompt.
    #[arg(long)]
    pub(crate) yes: bool,

    /// Ignore a named check re-run at apply time.
    ///
    /// `market plan` accepts the same flag, and a check skippable at plan time
    /// but fatal at apply is a plan that can be written and never sent.
    #[arg(long = "skip-check", value_name = "CHECK_ID")]
    pub(crate) skip_check: Vec<String>,

    /// Bypass embedded-ABI validation while rebuilding the plan before submission.
    #[arg(long)]
    pub(crate) skip_abi_check: bool,

    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}
