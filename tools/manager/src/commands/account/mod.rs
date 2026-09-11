mod add_key;
mod delete;
mod get;

pub use add_key::AddKey;
pub use delete::Delete;
pub use get::Get;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum AccountNs {
    /// Read an account's on-chain details.
    Get(Get),
    /// Add a full-access key to the signer account.
    AddKey(AddKey),
    /// Delete the signer account, sweeping its balance to a beneficiary.
    Delete(Delete),
}
