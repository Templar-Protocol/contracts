use std::path::PathBuf;

use clap::Args;
use near_account_id::AccountId;

/// Reconstruct a deployment spec from a deployed market.
///
/// Arguments only — the read chain that fulfills this lives in
/// [`crate::dispatch::export`], keeping this layer free of IO like every other
/// command.
#[derive(Args, Debug)]
pub struct Export {
    /// Deployed market account, e.g. `iethfxrp-ixlmusdc.templar-alpha.near`.
    #[arg(value_name = "MARKET_ID")]
    pub(crate) market_id: AccountId,

    /// Governance admin. No read method exposes it, so it cannot be recovered
    /// from chain state and must be stated.
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub(crate) governance_admin: AccountId,

    /// Write the spec here instead of stdout.
    #[arg(long, value_name = "PATH")]
    pub(crate) out: Option<PathBuf>,
}
