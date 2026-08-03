//! Adapter queries and administration commands.

use crate::{
    cli::{AdapterArgs, AdapterCommand},
    manifest::Manifest,
    stellar::CommandExecutor,
};

use super::{
    context::CommandContext,
    inventory::{args, selected_blend_adapter},
    invoke::invoke_response,
    output::Response,
};

pub(super) fn run_adapter<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &Manifest,
    adapter_args: &AdapterArgs,
) -> anyhow::Result<Response> {
    let stellar = context.stellar();
    let adapter = selected_blend_adapter(manifest, adapter_args)?;
    match &adapter_args.command {
        AdapterCommand::TotalAssets { asset } => invoke_response(stellar.invoke_view(
            adapter,
            "total_assets",
            args([("--asset", asset.as_str())]),
        )?),
        AdapterCommand::Admin => {
            invoke_response(stellar.invoke_view(adapter, "admin", Vec::new())?)
        }
        AdapterCommand::Vault => {
            invoke_response(stellar.invoke_view(adapter, "vault", Vec::new())?)
        }
        AdapterCommand::Pool => {
            invoke_response(stellar.invoke_view(adapter, "pool", Vec::new())?)
        }
        AdapterCommand::SetAdmin { caller, admin } => invoke_response(stellar.invoke(
            adapter,
            "set_admin",
            args([("--caller", caller.as_str()), ("--admin", admin.as_str())]),
        )?),
        AdapterCommand::AcceptAdmin { caller } => invoke_response(stellar.invoke(
            adapter,
            "accept_admin",
            args([("--caller", caller.as_str())]),
        )?),
        AdapterCommand::ExtendTtl { caller } => invoke_response(stellar.invoke(
            adapter,
            "extend_ttl",
            args([("--caller", caller.as_str())]),
        )?),
    }
}
