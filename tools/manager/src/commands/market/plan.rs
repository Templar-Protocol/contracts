use std::path::PathBuf;

use clap::Args;
use near_account_id::AccountId;
use near_api::PublicKey as CliPublicKey;
use templar_gateway_types::primitive::PublicKey;

use crate::commands::signer::SignerArgs;

/// Generate the deployment for a spec as an editable file, without sending
/// anything.
///
/// Deliberately takes no credential. Planning reads the chain and writes a file;
/// giving it a key would mean a mistyped subcommand could spend NEAR. The
/// operator's account and public key are still needed — the account signs each
/// planned transaction, and the key is granted full access on every account the
/// deploy creates — but neither is a secret.
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
    public_key: CliPublicKey,

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
}

impl Plan {
    pub(crate) fn public_key(&self) -> PublicKey {
        PublicKey::from(self.public_key)
    }
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

    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}
