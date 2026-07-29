//! Curator and allocator command handling.

use templar_curator_proxy_soroban::AllocationDelta;
use templar_soroban_shared_types::VaultCommand as WireVaultCommand;

use crate::{cli::CuratorCommand, manifest::Manifest, stellar::CommandExecutor};

use super::{
    context::CommandContext,
    governance::submit_and_maybe_accept,
    invoke::{address_vec_json, supply_queue_entries_json},
    output::Response,
    vault_ops::{execute_allocation, execute_vault, required_amount},
};

pub(super) fn run_curator<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &Manifest,
    command: &CuratorCommand,
) -> anyhow::Result<Response> {
    let stellar = context.stellar();
    match command {
        CuratorCommand::AllocateSupply {
            caller,
            market,
            amount,
            amount_raw,
            asset_decimals,
        } => {
            let amount = required_amount("amount", amount.as_ref(), *amount_raw, *asset_decimals)?;
            execute_allocation(
                stellar,
                manifest,
                caller,
                &AllocationDelta::Supply(*market, amount),
            )
        }
        CuratorCommand::AllocateWithdraw {
            caller,
            market,
            amount,
            amount_raw,
            asset_decimals,
        } => {
            let amount = required_amount("amount", amount.as_ref(), *amount_raw, *asset_decimals)?;
            execute_allocation(
                stellar,
                manifest,
                caller,
                &AllocationDelta::Withdraw(*market, amount),
            )
        }
        CuratorCommand::AbortWithdrawing { caller, op_id } => execute_vault(
            stellar,
            manifest,
            WireVaultCommand::AbortWithdrawing {
                caller: caller.to_string(),
                op_id: *op_id,
            },
        ),
        CuratorCommand::RefreshMarkets { caller, markets } => execute_vault(
            stellar,
            manifest,
            WireVaultCommand::RefreshMarkets {
                caller: caller.to_string(),
                markets: markets.clone(),
            },
        ),
        CuratorCommand::RefreshFees => {
            execute_vault(stellar, manifest, WireVaultCommand::RefreshFees)
        }
        CuratorCommand::ResyncIdle => {
            execute_vault(stellar, manifest, WireVaultCommand::ResyncIdleBalance)
        }
        CuratorCommand::SetAllowedAdapters {
            admin,
            adapters,
            auto_accept,
        } => submit_and_maybe_accept(
            stellar,
            manifest,
            admin.as_str(),
            "submit_set_allowed_adapters",
            vec![
                "--caller".to_string(),
                admin.to_string(),
                "--adapters".to_string(),
                address_vec_json(adapters)?,
            ],
            *auto_accept,
        ),
        CuratorCommand::SetSupplyQueue {
            admin,
            entries,
            auto_accept,
        } => submit_and_maybe_accept(
            stellar,
            manifest,
            admin.as_str(),
            "submit_set_supply_queue",
            vec![
                "--caller".to_string(),
                admin.to_string(),
                "--entries".to_string(),
                supply_queue_entries_json(entries)?,
            ],
            *auto_accept,
        ),
    }
}
