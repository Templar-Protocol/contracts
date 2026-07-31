use std::path::PathBuf;

use clap::Args;
use near_account_id::AccountId;

/// Re-run the preflight against deployed state.
///
/// Takes no signer: it only reads. Exits non-zero when any check fails, so it
/// can run on a schedule against live markets.
#[derive(Args, Debug)]
pub struct Verify {
    /// The deployed market to check.
    pub(crate) market_id: AccountId,

    /// The account holding the governance Admin role.
    ///
    /// Not recoverable from chain state — the role is granted at init and the
    /// contract exposes no view naming its holder — so it is supplied rather
    /// than guessed, exactly as `market export` requires it.
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub(crate) governance_admin: AccountId,

    /// Compare deployed state against an intended spec.
    #[arg(long, value_name = "PATH")]
    pub(crate) against: Option<PathBuf>,

    /// Accept a `decimals` override that disagrees with the token's metadata.
    #[arg(long)]
    pub(crate) accept_decimals_mismatch: bool,
}
