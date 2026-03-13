pub mod create;
pub mod deploy;
pub mod governance;
pub mod proxy;
pub mod remove;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct ProxyOracleArgs {
    #[command(subcommand)]
    command: ProxyOracleCommand,
}

#[derive(clap::Subcommand, Debug)]
enum ProxyOracleCommand {
    /// Deploy a proxy oracle from a registry
    Create(create::CreateProxyOracle),

    /// Deploy a proxy oracle contract directly from a WASM file
    Deploy(deploy::DeployProxyOracle),

    /// Delete a proxy oracle account
    Remove(remove::ProxyOracleRemove),

    /// Query proxies configured on a proxy oracle
    Proxy(proxy::ProxyArgs),

    /// Manage governance proposals on a proxy oracle
    Governance(governance::GovernanceArgs),
}

impl ProxyOracleArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            ProxyOracleCommand::Create(a) => a.run(ctx).await,
            ProxyOracleCommand::Deploy(a) => a.run(ctx).await,
            ProxyOracleCommand::Remove(a) => a.run(ctx).await,
            ProxyOracleCommand::Proxy(a) => a.run(ctx).await,
            ProxyOracleCommand::Governance(a) => a.run(ctx).await,
        }
    }
}
