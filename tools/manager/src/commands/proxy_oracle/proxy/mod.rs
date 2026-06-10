pub mod get;
pub mod list;

use std::fmt;
use std::str::FromStr;

use templar_common::oracle::pyth::PriceIdentifier;

use crate::CliContext;

/// Newtype around [`PriceIdentifier`] that implements [`FromStr`] for use as a clap argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CliPriceIdentifier(PriceIdentifier);

impl From<CliPriceIdentifier> for PriceIdentifier {
    fn from(value: CliPriceIdentifier) -> Self {
        value.0
    }
}

impl From<PriceIdentifier> for CliPriceIdentifier {
    fn from(value: PriceIdentifier) -> Self {
        Self(value)
    }
}

impl fmt::Display for CliPriceIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for CliPriceIdentifier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(s).map_err(|e| format!("invalid hex: {e}"))?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|v: Vec<u8>| format!("expected 32 bytes, got {}", v.len()))?;
        Ok(Self(PriceIdentifier(bytes)))
    }
}

#[derive(clap::Args, Debug)]
pub struct ProxyArgs {
    #[command(subcommand)]
    command: ProxyCommand,
}

#[derive(clap::Subcommand, Debug)]
enum ProxyCommand {
    /// List all proxy price identifiers
    List(list::ListProxies),

    /// Get details of a specific proxy by price identifier
    Get(get::GetProxy),
}

impl ProxyArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            ProxyCommand::List(a) => a.run(ctx).await,
            ProxyCommand::Get(a) => a.run(ctx).await,
        }
    }
}
