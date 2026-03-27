pub mod cancel;
pub mod create;
pub mod execute;
pub mod get;
pub mod list;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct GovernanceArgs {
    #[command(subcommand)]
    command: GovernanceCommand,
}

#[derive(clap::Subcommand, Debug)]
enum GovernanceCommand {
    /// List all active governance proposals
    List(list::ListProposals),

    /// Get details of a specific governance proposal
    Get(get::GetProposal),

    /// Create a new governance proposal
    Create(create::CreateProposal),

    /// Cancel an active governance proposal
    Cancel(cancel::CancelProposal),

    /// Execute a governance proposal whose TTL has elapsed
    Execute(execute::ExecuteProposal),
}

impl GovernanceArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            GovernanceCommand::List(a) => a.run(ctx).await,
            GovernanceCommand::Get(a) => a.run(ctx).await,
            GovernanceCommand::Create(a) => a.run(ctx).await,
            GovernanceCommand::Cancel(a) => a.run(ctx).await,
            GovernanceCommand::Execute(a) => a.run(ctx).await,
        }
    }
}
