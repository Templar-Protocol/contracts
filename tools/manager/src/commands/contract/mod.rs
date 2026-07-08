mod get_version;

pub use get_version::GetVersion;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ContractNs {
    /// Read a deployed contract's registered version.
    GetVersion(GetVersion),
}
