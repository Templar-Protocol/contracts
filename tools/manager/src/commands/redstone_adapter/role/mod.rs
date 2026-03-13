pub mod list;
pub mod set;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct RoleArgs {
    #[command(subcommand)]
    command: RoleCommand,
}

#[derive(clap::Subcommand, Debug)]
enum RoleCommand {
    /// List accounts that have a specific role
    List(list::RoleList),
    /// Grant or revoke a role for an account
    Set(set::RoleSet),
}

impl RoleArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            RoleCommand::List(a) => a.run(ctx).await,
            RoleCommand::Set(a) => a.run(ctx).await,
        }
    }
}
