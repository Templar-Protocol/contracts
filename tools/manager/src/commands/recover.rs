use clap::Args;
use near_account_id::AccountId;

use crate::commands::signer::SignerArgs;

/// Recover a NEP-141 balance from the signer account to a beneficiary, then
/// unregister the signer's storage. The signer is both the source of the tokens
/// and the account whose storage is unregistered.
#[derive(Args, Debug)]
pub struct RecoverNep141 {
    /// NEP-141 token contract to recover the signer's balance from.
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub token_id: AccountId,
    /// Account that receives the recovered tokens.
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub beneficiary_id: AccountId,
    /// Forwarded to `storage_unregister(force = …)`; only affects unregistration.
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}
