mod delete;
mod get;

pub use delete::Delete;
pub use get::Get;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum AccountNs {
    Get(Get),
    /// Delete the signer account, sweeping its balance to a beneficiary.
    Delete(Delete),
}
