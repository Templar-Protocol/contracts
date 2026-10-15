//! User-facing vault and share operations.

use std::io::IsTerminal as _;

use anyhow::Context;
use templar_soroban_shared_types::VaultCommand as WireVaultCommand;

use crate::{
    cli::UserCommand, manifest::Manifest, stellar::CommandExecutor, types::raw_to_decimal,
};

use super::{
    context::CommandContext,
    inventory::{args, contract_id, required_contract},
    invoke::invoke_response,
    output::Response,
    safety::prompt_yes,
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
            let from_display = assets.is_some();
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
            confirm_deposit(
                context,
                manifest,
                assets,
                *asset_decimals,
                from_display,
                &format!("from {operator} to {receiver}; min shares out {min_shares_out} (raw)"),
            )?;
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
        } => {
            let assets = required_amount("assets", assets.as_ref(), *assets_raw, *asset_decimals)?;
            let owner = owner.as_ref().unwrap_or(operator);
            let receiver = receiver.as_ref().unwrap_or(operator);
            let proxy = required_contract(manifest, "proxy_4626")?;
            invoke_response(stellar.invoke(
                proxy,
                "withdraw",
                args([
                    ("--operator", operator.as_str()),
                    ("--assets", &assets.to_string()),
                    ("--receiver", receiver.as_str()),
                    ("--owner", owner.as_str()),
                ]),
            )?)
        }
        UserCommand::Redeem {
            operator,
            receiver,
            owner,
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
            let owner = owner.as_ref().unwrap_or(operator);
            let receiver = receiver.as_ref().unwrap_or(operator);
            let proxy = required_contract(manifest, "proxy_4626")?;
            invoke_response(stellar.invoke(
                proxy,
                "redeem",
                args([
                    ("--operator", operator.as_str()),
                    ("--shares", &shares.to_string()),
                    ("--receiver", receiver.as_str()),
                    ("--owner", owner.as_str()),
                ]),
            )?)
        }
        UserCommand::AtomicWithdraw {
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
            if let Some(proxy) = contract_id(manifest, "proxy_4626") {
                require_atomic_exit_proxy(context, proxy)?;
                invoke_response(stellar.invoke(
                    proxy,
                    "atomic_withdraw",
                    args([
                        ("--operator", operator.as_str()),
                        ("--assets", &assets.to_string()),
                        ("--receiver", receiver.as_str()),
                        ("--owner", owner.as_str()),
                        ("--max_shares_burned", &max_shares_burned.to_string()),
                    ]),
                )?)
            } else {
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
        }
        UserCommand::AtomicRedeem {
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
            if let Some(proxy) = contract_id(manifest, "proxy_4626") {
                require_atomic_exit_proxy(context, proxy)?;
                invoke_response(stellar.invoke(
                    proxy,
                    "atomic_redeem",
                    args([
                        ("--operator", operator.as_str()),
                        ("--shares", &shares.to_string()),
                        ("--receiver", receiver.as_str()),
                        ("--owner", owner.as_str()),
                        ("--min_assets_out", &min_assets_out.to_string()),
                    ]),
                )?)
            } else {
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

/// Shows the deposit in display units before it is signed, so a base-unit amount
/// passed to `--assets` (scaled by 10^decimals) is visible before funds move.
/// Refuses when `--assets` was converted with decimals the asset token disagrees with.
fn confirm_deposit<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &Manifest,
    raw: i128,
    asset_decimals: u32,
    from_display: bool,
    parties: &str,
) -> anyhow::Result<()> {
    let cli = context.cli();
    let on_chain_decimals = if cli.dry_run {
        None
    } else {
        // The manifest's asset token, else the one the vault itself reports.
        contract_id(manifest, "asset_token")
            .map(str::to_owned)
            .or_else(|| {
                let vault = contract_id(manifest, "vault")?;
                crate::commands::upgrade::read_vault_view(context.stellar(), vault)?.asset_token_id
            })
            .and_then(|token| {
                context
                    .stellar()
                    .invoke_view(&token, "decimals", Vec::new())
                    .ok()
            })
            .and_then(|out| out.stdout.trim().trim_matches('"').parse::<u32>().ok())
    };
    // `--assets` display amounts must be converted with the asset token's own decimals;
    // a conversion that cannot be verified is refused rather than trusted.
    if from_display && !cli.dry_run {
        let actual = on_chain_decimals.context(
            "cannot read the asset token's decimals to verify the --assets conversion; \
             use --assets-raw with base units instead",
        )?;
        anyhow::ensure!(
            actual == asset_decimals,
            "--assets was converted with --asset-decimals {asset_decimals}, but the asset token \
             reports {actual} decimals; pass --asset-decimals {actual} or use --assets-raw"
        );
    }
    if cli.json || cli.json_lines {
        return Ok(());
    }
    let decimals = on_chain_decimals.unwrap_or(asset_decimals);
    let source = if on_chain_decimals.is_some() {
        "asset token decimals"
    } else {
        "--asset-decimals; asset token decimals unread"
    };
    eprintln!(
        "Deposit: {} assets ({raw} base units, {decimals} decimals from {source}) {parties}",
        raw_to_decimal(raw, decimals)
    );
    if cli.yes || cli.dry_run || !std::io::stdin().is_terminal() {
        return Ok(());
    }
    prompt_yes("deposit cancelled; pass --yes to skip this confirmation")
}

fn require_atomic_exit_proxy<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    proxy: &str,
) -> anyhow::Result<()> {
    const REQUIRED_FUNCTIONS: [&str; 2] = ["atomic_withdraw", "atomic_redeem"];

    if context.cli().dry_run {
        eprintln!(
            "Warning: dry-run cannot verify ERC-4626 proxy {proxy} atomic-exit support; the planned call remains routed through the recorded proxy"
        );
        return Ok(());
    }

    let available = context
        .stellar()
        .contract_interface_functions(proxy)
        .with_context(|| {
            format!(
                "cannot verify atomic-exit support for recorded ERC-4626 proxy {proxy}; refusing direct-vault fallback while proxy_4626 is present"
            )
        })?;
    let missing = REQUIRED_FUNCTIONS
        .into_iter()
        .filter(|function| !available.contains(*function))
        .collect::<Vec<_>>();
    anyhow::ensure!(
        missing.is_empty(),
        "recorded ERC-4626 proxy {proxy} is legacy and does not expose {}; deploy a replacement proxy and update the proxy_4626 manifest record before retrying",
        missing.join(" and ")
    );
    Ok(())
}
