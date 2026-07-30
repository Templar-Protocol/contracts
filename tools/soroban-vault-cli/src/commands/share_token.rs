//! Share-token queries and maintenance commands.

use crate::{cli::ShareTokenCommand, manifest::Manifest, stellar::CommandExecutor};

use super::{
    context::CommandContext,
    inventory::{args, required_contract},
    invoke::invoke_response,
    output::Response,
};

pub(super) fn run_share_token<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &Manifest,
    command: &ShareTokenCommand,
) -> anyhow::Result<Response> {
    let stellar = context.stellar();
    let share = required_contract(manifest, "share_token")?;
    match command {
        ShareTokenCommand::Balance { account } => invoke_response(stellar.invoke_view(
            share,
            "balance",
            args([("--account", account.as_str())]),
        )?),
        ShareTokenCommand::TotalSupply => {
            invoke_response(stellar.invoke_view(share, "total_supply", Vec::new())?)
        }
        ShareTokenCommand::Admin => {
            invoke_response(stellar.invoke_view(share, "admin", Vec::new())?)
        }
        ShareTokenCommand::Vault => {
            invoke_response(stellar.invoke_view(share, "vault", Vec::new())?)
        }
        ShareTokenCommand::ExtendTtl { caller } => invoke_response(stellar.invoke(
            share,
            "extend_ttl",
            args([("--caller", caller.as_str())]),
        )?),
    }
}
