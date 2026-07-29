//! User-facing vault and share operations.

use anyhow::Context;
use templar_soroban_shared_types::VaultCommand as WireVaultCommand;

use crate::{cli::UserCommand, manifest::Manifest, stellar::CommandExecutor};

use super::{
    context::CommandContext,
    inventory::{args, contract_id, required_contract},
    invoke::invoke_response,
    output::Response,
    vault_ops::{
        execute_vault, optional_amount, optional_share_amount, required_amount,
        required_share_amount,
    },
};

#[allow(
    clippy::too_many_lines,
    reason = "keeps user command routing local and explicit"
)]
pub(super) fn run_user<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &Manifest,
    command: &UserCommand,
) -> anyhow::Result<Response> {
    let stellar = context.stellar();
    match command {
        UserCommand::Deposit {
            operator,
            receiver,
            assets,
            assets_raw,
            asset_decimals,
            min_shares_out,
            min_shares_out_raw,
            share_decimals,
        } => {
            let assets = required_amount("assets", assets.as_ref(), *assets_raw, *asset_decimals)?;
            let min_shares_out = optional_share_amount(
                manifest,
                "min_shares_out",
                min_shares_out.as_ref(),
                Some(*min_shares_out_raw),
                *share_decimals,
            )?
            .unwrap_or(0);
            let receiver = receiver.as_ref().unwrap_or(operator);
            if let Some(proxy) = contract_id(manifest, "proxy_4626") {
                invoke_response(stellar.invoke(
                    proxy,
                    "deposit_with_min",
                    args([
                        ("--operator", operator.as_str()),
                        ("--assets", &assets.to_string()),
                        ("--receiver", receiver.as_str()),
                        ("--min_shares_out", &min_shares_out.to_string()),
                    ]),
                )?)
            } else {
                execute_vault(
                    stellar,
                    manifest,
                    WireVaultCommand::DepositWithMin {
                        owner: operator.to_string(),
                        receiver: receiver.to_string(),
                        assets,
                        min_shares_out,
                    },
                )
            }
        }
        UserCommand::Mint {
            operator,
            receiver,
            shares,
            shares_raw,
            share_decimals,
        } => {
            let shares = required_share_amount(
                manifest,
                "shares",
                shares.as_ref(),
                *shares_raw,
                *share_decimals,
            )?;
            let receiver = receiver.as_ref().unwrap_or(operator);
            let proxy = required_contract(manifest, "proxy_4626")?;
            invoke_response(stellar.invoke(
                proxy,
                "mint",
                args([
                    ("--operator", operator.as_str()),
                    ("--shares", &shares.to_string()),
                    ("--receiver", receiver.as_str()),
                ]),
            )?)
        }
        UserCommand::Withdraw {
            operator,
            receiver,
            owner,
            assets,
            assets_raw,
            asset_decimals,
            max_shares_burned,
            max_shares_burned_raw,
            share_decimals,
        } => {
            let assets = required_amount("assets", assets.as_ref(), *assets_raw, *asset_decimals)?;
            let max_shares_burned = optional_share_amount(
                manifest,
                "max_shares_burned",
                max_shares_burned.as_ref(),
                *max_shares_burned_raw,
                *share_decimals,
            )?
            .unwrap_or(assets);
            let owner = owner.as_ref().unwrap_or(operator);
            let receiver = receiver.as_ref().unwrap_or(operator);
            execute_vault(
                stellar,
                manifest,
                WireVaultCommand::AtomicWithdraw {
                    owner: owner.to_string(),
                    receiver: receiver.to_string(),
                    operator: operator.to_string(),
                    assets,
                    max_shares_burned,
                },
            )
        }
        UserCommand::Redeem {
            operator,
            receiver,
            owner,
            shares,
            shares_raw,
            share_decimals,
            min_assets_out,
            min_assets_out_raw,
            asset_decimals,
        } => {
            let shares = required_share_amount(
                manifest,
                "shares",
                shares.as_ref(),
                *shares_raw,
                *share_decimals,
            )?;
            let min_assets_out = optional_amount(
                "min_assets_out",
                min_assets_out.as_ref(),
                Some(*min_assets_out_raw),
                *asset_decimals,
            )?;
            let owner = owner.as_ref().unwrap_or(operator);
            let receiver = receiver.as_ref().unwrap_or(operator);
            execute_vault(
                stellar,
                manifest,
                WireVaultCommand::AtomicRedeem {
                    owner: owner.to_string(),
                    receiver: receiver.to_string(),
                    operator: operator.to_string(),
                    shares,
                    min_assets_out,
                },
            )
        }
        UserCommand::RequestWithdraw {
            owner,
            receiver,
            shares,
            shares_raw,
            share_decimals,
            min_assets_out,
            min_assets_out_raw,
            asset_decimals,
        } => {
            let shares = required_share_amount(
                manifest,
                "shares",
                shares.as_ref(),
                *shares_raw,
                *share_decimals,
            )?;
            let min_assets_out = optional_amount(
                "min_assets_out",
                min_assets_out.as_ref(),
                Some(*min_assets_out_raw),
                *asset_decimals,
            )?;
            let receiver = receiver.as_ref().unwrap_or(owner);
            execute_vault(
                stellar,
                manifest,
                WireVaultCommand::RequestWithdraw {
                    owner: owner.to_string(),
                    receiver: receiver.to_string(),
                    shares,
                    min_assets_out,
                },
            )
        }
        UserCommand::ExecuteWithdraw { operator } => {
            if let Some(proxy) = contract_id(manifest, "proxy_4626") {
                invoke_response(stellar.invoke(
                    proxy,
                    "execute_withdraw",
                    args([("--operator", operator.as_str())]),
                )?)
            } else {
                execute_vault(
                    stellar,
                    manifest,
                    WireVaultCommand::ExecuteWithdraw {
                        caller: operator.to_string(),
                    },
                )
            }
        }
        UserCommand::Balance { owner } => {
            let share = required_contract(manifest, "share_token")?;
            invoke_response(stellar.invoke_view(
                share,
                "balance",
                args([("--account", owner.as_str())]),
            )?)
        }
        UserCommand::Preview {
            owner,
            assets,
            assets_raw,
            asset_decimals,
            shares,
            shares_raw,
            share_decimals,
        }
        | UserCommand::View {
            owner,
            assets,
            assets_raw,
            asset_decimals,
            shares,
            shares_raw,
            share_decimals,
        } => {
            let assets = optional_amount(
                "assets",
                assets.as_ref(),
                Some(*assets_raw),
                *asset_decimals,
            )?;
            let shares = optional_share_amount(
                manifest,
                "shares",
                shares.as_ref(),
                Some(*shares_raw),
                *share_decimals,
            )?
            .unwrap_or(0);
            let target = contract_id(manifest, "proxy_4626")
                .or_else(|| contract_id(manifest, "vault"))
                .context("missing proxy_4626 or vault contract id in manifest")?;
            let function = if contract_id(manifest, "proxy_4626").is_some() {
                "preview"
            } else {
                "proxy_view"
            };
            invoke_response(stellar.invoke_view(
                target,
                function,
                args([
                    ("--owner", owner.as_str()),
                    ("--assets", &assets.to_string()),
                    ("--shares", &shares.to_string()),
                ]),
            )?)
        }
    }
}
