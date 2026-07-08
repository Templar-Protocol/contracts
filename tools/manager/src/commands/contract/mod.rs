mod get_version;

pub use get_version::GetVersion;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ContractNs {
    GetVersion(GetVersion),
}
