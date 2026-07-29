//! Fail-closed command and governance safety checks.

use std::io::{self, Write as _};

use crate::{
    cli::{Cli, Commands, DeployCommand, DeployPlanCommand, GovernanceCommand},
    manifest::Manifest,
    stellar::{CommandExecutor, Stellar},
    types::AddressStr,
};

use super::{
    context::CommandContext,
    inventory::{args, contract_id, required_contract},
    invoke::{option_i128_arg, supply_queue_entries_json},
};

pub(super) fn guard_write(cli: &Cli) -> anyhow::Result<()> {
    if cli.command.is_write() && cli.network == "mainnet" && !cli.allow_mainnet_write {
        anyhow::bail!("mainnet write blocked; pass --allow-mainnet-write to continue");
    }
    Ok(())
}

pub(super) fn guard_fresh_state_usage(cli: &Cli) -> anyhow::Result<()> {
    if !cli.fresh_state {
        return Ok(());
    }
    let supported = match &cli.command {
        Commands::Deploy(args) => match &args.command {
            DeployCommand::Stack(_) => true,
            DeployCommand::Plan(plan) => matches!(plan.command, DeployPlanCommand::Stack(_)),
            _ => false,
        },
        _ => false,
    };
    anyhow::ensure!(
        supported,
        "--fresh-state is only valid with `deploy stack` or `deploy plan stack`"
    );
    Ok(())
}

pub(super) fn confirm_dangerous_governance_change<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &Manifest,
) -> anyhow::Result<()> {
    let cli = context.cli();
    let stellar = context.stellar();
    let Some(diff) = governance_safety_diff(stellar, manifest, &cli.command)? else {
        return Ok(());
    };
    if cli.json || cli.json_lines {
        anyhow::ensure!(
            cli.yes || cli.dry_run,
            "dangerous governance change requires --yes in machine-readable output mode"
        );
        return Ok(());
    }
    eprintln!("Dangerous governance change: {}", diff.title);
    for line in diff.lines {
        eprintln!("  {line}");
    }
    if cli.yes || cli.dry_run {
        return Ok(());
    }

    eprint!("Continue? Type 'yes' to submit: ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    anyhow::ensure!(
        matches!(answer.trim(), "yes" | "y"),
        "operation cancelled; pass --yes to confirm after reviewing the semantic diff"
    );
    Ok(())
}

fn governance_safety_diff<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    command: &Commands,
) -> anyhow::Result<Option<SafetyDiff>> {
    let Commands::Governance(governance_args) = command else {
        return Ok(None);
    };
    let governance = required_contract(manifest, "governance")?;
    let diff = match &governance_args.command {
        GovernanceCommand::SubmitSetAdmin { new_admin, .. } => SafetyDiff {
            title: "admin rotation".to_string(),
            lines: vec![format!(
                "admin: {} -> {}",
                view_or_unavailable(stellar, governance, "admin", Vec::new()),
                new_admin
            )],
        },
        GovernanceCommand::SubmitSetTimelock {
            kind, timelock_ns, ..
        } => SafetyDiff {
            title: "timelock update".to_string(),
            lines: vec![format!(
                "{kind} timelock_ns: {} -> {}",
                view_or_unavailable(
                    stellar,
                    governance,
                    "timelock_ns",
                    args([("--kind", &kind.to_string())]),
                ),
                timelock_ns
            )],
        },
        GovernanceCommand::SubmitSetSupplyQueue { admin, entries } => SafetyDiff {
            title: "supply queue replacement".to_string(),
            lines: vec![
                format!(
                    "current supply queue view: {}",
                    current_vault_view(stellar, manifest, admin)
                ),
                format!(
                    "proposed supply queue: {}",
                    supply_queue_entries_json(entries)?
                ),
            ],
        },
        GovernanceCommand::SubmitSetFees {
            admin,
            performance_fee_wad,
            performance_recipient,
            management_fee_wad,
            management_recipient,
            max_growth_rate_wad,
        } => SafetyDiff {
            title: "fee parameter update".to_string(),
            lines: vec![
                format!("current fee view: {}", current_vault_view(stellar, manifest, admin)),
                format!(
                    "proposed fees: performance_fee_wad={} performance_recipient={} management_fee_wad={} management_recipient={} max_growth_rate_wad={}",
                    performance_fee_wad,
                    performance_recipient,
                    management_fee_wad,
                    management_recipient,
                    option_i128_arg(*max_growth_rate_wad),
                ),
            ],
        },
        _ => return Ok(None),
    };
    Ok(Some(diff))
}

pub(super) fn view_or_unavailable<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    contract_id: &str,
    function: &str,
    args: Vec<String>,
) -> String {
    match stellar.invoke_view(contract_id, function, args) {
        Ok(output) if !output.stdout.is_empty() => output.stdout,
        Ok(_) => "<empty>".to_string(),
        Err(error) => format!("unavailable ({error})"),
    }
}

pub(super) fn current_vault_view<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    owner: &AddressStr,
) -> String {
    let Some(vault) = contract_id(manifest, "vault") else {
        return "unavailable (missing vault contract id in manifest)".to_string();
    };
    view_or_unavailable(
        stellar,
        vault,
        "proxy_view",
        args([
            ("--owner", owner.as_str()),
            ("--assets", "0"),
            ("--shares", "0"),
        ]),
    )
}

struct SafetyDiff {
    title: String,
    lines: Vec<String>,
}
