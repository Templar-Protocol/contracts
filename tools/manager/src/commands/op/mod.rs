mod get;

pub use get::Get;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum OpNs {
    Get(Get),
}
