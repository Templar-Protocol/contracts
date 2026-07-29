//! Governance proposal, queue, acceptance, and inspection commands.

use std::{
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    cli::{GovernanceCommand, GovernanceSubmitAndWaitCommand},
    manifest::Manifest,
    stellar::{CommandExecutor, Stellar},
    types::{AddressStr, FeeParamsArg},
};
use templar_soroban_governance::GovernanceActionKind;

use super::{
    context::CommandContext,
    deploy::stellar_command_shape,
    inventory::{args, parse_proposal_id, required_contract},
    invoke::{address_vec_json, invoke_response, option_i128_arg, supply_queue_entries_json},
    output::{
        GovernanceAcceptReadyResponse, GovernanceProposalView, GovernanceQueueResponse,
        PlanResponse, Response,
    },
};

#[allow(
    clippy::too_many_lines,
    reason = "keeps governance method names and typed argument routing visibly aligned with the contract ABI"
)]
pub(super) fn run_governance<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &Manifest,
    command: &GovernanceCommand,
) -> anyhow::Result<Response> {
    let stellar = context.stellar();
    let governance = required_contract(manifest, "governance")?;
    match command {
        GovernanceCommand::PlanAccept { admin, proposal_id } => {
            Ok(Response::Plan(governance_plan(
                "governance accept",
                &manifest.network,
                vec![admin.to_string()],
                vec![stellar_command_shape(
                    &format!(
                        "contract invoke --id {governance} -- accept --caller {admin} --proposal_id {proposal_id}"
                    ),
                    true,
                )],
            )))
        }
        GovernanceCommand::PlanSubmitSetSupplyQueue { admin, entries } => {
            let entries_json = supply_queue_entries_json(entries)?;
            Ok(Response::Plan(governance_plan(
                "governance submit-set-supply-queue",
                &manifest.network,
                vec![admin.to_string()],
                vec![stellar_command_shape(
                    &format!(
                        "contract invoke --id {governance} -- submit_set_supply_queue --caller {admin} --entries '{entries_json}'"
                    ),
                    true,
                )],
            )))
        }
        GovernanceCommand::PlanSubmitSetTimelock {
            admin,
            kind,
            timelock_ns,
        } => Ok(Response::Plan(governance_plan(
            "governance submit-set-timelock",
            &manifest.network,
            vec![admin.to_string()],
            vec![stellar_command_shape(
                &format!(
                    "contract invoke --id {governance} -- submit_set_timelock --caller {admin} --kind {kind} --new_timelock_ns {timelock_ns}"
                ),
                true,
            )],
        ))),
        GovernanceCommand::Queue { kind } => {
            let queue = governance_queue(stellar, governance, kind.as_ref())?;
            Ok(Response::GovernanceQueue(queue))
        }
        GovernanceCommand::Explain { proposal_id } => {
            let proposal = inspect_governance_proposal(stellar, governance, *proposal_id)?;
            Ok(Response::GovernanceExplain(proposal))
        }
        GovernanceCommand::AcceptReady { admin, kind, limit } => {
            run_governance_accept_ready(stellar, governance, admin, kind.as_ref(), *limit)
        }
        GovernanceCommand::SubmitAndWait(args) => {
            run_governance_submit_and_wait(stellar, governance, args)
        }
        GovernanceCommand::Accept { admin, proposal_id } => invoke_response(stellar.invoke(
            governance,
            "accept",
            args([
                ("--caller", admin.as_str()),
                ("--proposal_id", &proposal_id.to_string()),
            ]),
        )?),
        GovernanceCommand::Revoke { admin, proposal_id } => invoke_response(stellar.invoke(
            governance,
            "revoke",
            args([
                ("--caller", admin.as_str()),
                ("--proposal_id", &proposal_id.to_string()),
            ]),
        )?),
        GovernanceCommand::Pending { proposal_id } => {
            if let Some(proposal_id) = proposal_id {
                invoke_response(stellar.invoke_view(
                    governance,
                    "pending",
                    args([("--proposal_id", &proposal_id.to_string())]),
                )?)
            } else {
                invoke_response(stellar.invoke_view(governance, "pending_ids", Vec::new())?)
            }
        }
        GovernanceCommand::Timelocks => {
            invoke_response(stellar.invoke_view(governance, "timelocks", Vec::new())?)
        }
        GovernanceCommand::SubmitSetAdmin { admin, new_admin } => invoke_response(stellar.invoke(
            governance,
            "submit_set_admin",
            args([
                ("--caller", admin.as_str()),
                ("--new_admin", new_admin.as_str()),
            ]),
        )?),
        GovernanceCommand::SubmitSetCurator { admin, new_curator } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_curator",
                args([
                    ("--caller", admin.as_str()),
                    ("--new_curator", new_curator.as_str()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSetGovernance {
            admin,
            new_governance,
        } => invoke_response(stellar.invoke(
            governance,
            "submit_set_governance",
            args([
                ("--caller", admin.as_str()),
                ("--governance", new_governance.as_str()),
            ]),
        )?),
        GovernanceCommand::SubmitSetPaused { admin, paused } => invoke_response(stellar.invoke(
            governance,
            "submit_set_paused",
            args([
                ("--caller", admin.as_str()),
                ("--paused", &paused.to_string()),
            ]),
        )?),
        GovernanceCommand::SubmitSetSupplyQueue { admin, entries } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_supply_queue",
                vec![
                    "--caller".to_string(),
                    admin.to_string(),
                    "--entries".to_string(),
                    supply_queue_entries_json(entries)?,
                ],
            )?)
        }
        GovernanceCommand::SubmitSetFees {
            admin,
            performance_fee_wad,
            performance_recipient,
            management_fee_wad,
            management_recipient,
            max_growth_rate_wad,
        } => {
            let fees = FeeParamsArg {
                performance_fee_wad: *performance_fee_wad,
                performance_recipient: performance_recipient.clone(),
                management_fee_wad: *management_fee_wad,
                management_recipient: management_recipient.clone(),
                max_growth_rate_wad: *max_growth_rate_wad,
            };
            invoke_response(stellar.invoke(
                governance,
                "submit_set_fees",
                args([
                    (
                        "--performance_fee_wad",
                        &fees.performance_fee_wad.to_string(),
                    ),
                    (
                        "--performance_recipient",
                        fees.performance_recipient.as_str(),
                    ),
                    ("--management_fee_wad", &fees.management_fee_wad.to_string()),
                    ("--management_recipient", fees.management_recipient.as_str()),
                    (
                        "--max_growth_rate_wad",
                        &option_i128_arg(fees.max_growth_rate_wad),
                    ),
                    ("--caller", admin.as_str()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSetRestrictions {
            admin,
            mode,
            accounts,
        } => invoke_response(stellar.invoke(
            governance,
            "submit_set_restrictions",
            vec![
                "--caller".to_string(),
                admin.to_string(),
                "--mode".to_string(),
                mode.as_u32().to_string(),
                "--accounts".to_string(),
                address_vec_json(accounts)?,
            ],
        )?),
        GovernanceCommand::SubmitSetSentinel { admin, sentinel } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_sentinel",
                args([
                    ("--caller", admin.as_str()),
                    ("--sentinel", sentinel.as_str()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSetAllocators { admin, allocators } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_allocators",
                vec![
                    "--caller".to_string(),
                    admin.to_string(),
                    "--allocators".to_string(),
                    address_vec_json(allocators)?,
                ],
            )?)
        }
        GovernanceCommand::SubmitSetAllowedAdapters { admin, adapters } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_allowed_adapters",
                vec![
                    "--caller".to_string(),
                    admin.to_string(),
                    "--adapters".to_string(),
                    address_vec_json(adapters)?,
                ],
            )?)
        }
        GovernanceCommand::SubmitSetTimelock {
            admin,
            kind,
            timelock_ns,
        } => invoke_response(stellar.invoke(
            governance,
            "submit_set_timelock",
            args([
                ("--caller", admin.as_str()),
                ("--kind", &kind.to_string()),
                ("--new_timelock_ns", &timelock_ns.to_string()),
            ]),
        )?),
        GovernanceCommand::SubmitSetCap {
            admin,
            market_id,
            cap,
        } => invoke_response(stellar.invoke(
            governance,
            "submit_set_cap",
            args([
                ("--caller", admin.as_str()),
                ("--market_id", &market_id.to_string()),
                ("--new_cap", &cap.to_string()),
            ]),
        )?),
        GovernanceCommand::SubmitRemoveMarket { admin, market_id } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_remove_market",
                args([
                    ("--caller", admin.as_str()),
                    ("--market_id", &market_id.to_string()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSetGroupCap { admin, group, cap } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_group_cap",
                args([
                    ("--caller", admin.as_str()),
                    ("--cap_group_id", group),
                    ("--new_cap", &cap.to_string()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSetGroupRelCap {
            admin,
            group,
            relative_cap,
        } => invoke_response(stellar.invoke(
            governance,
            "submit_set_group_rel_cap",
            args([
                ("--caller", admin.as_str()),
                ("--cap_group_id", group),
                ("--new_relative_cap_wad", &relative_cap.to_string()),
            ]),
        )?),
        GovernanceCommand::SubmitSetGroupMember {
            admin,
            market_id,
            group,
        } => invoke_response(stellar.invoke(
            governance,
            "submit_set_group_member",
            args([
                ("--caller", admin.as_str()),
                ("--market_id", &market_id.to_string()),
                ("--cap_group_id", group),
            ]),
        )?),
        GovernanceCommand::SubmitSetSkimRecipient { admin, recipient } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_skim_recipient",
                args([
                    ("--caller", admin.as_str()),
                    ("--recipient", recipient.as_str()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSkim { admin, token } => invoke_response(stellar.invoke(
            governance,
            "submit_skim",
            args([("--caller", admin.as_str()), ("--token", token.as_str())]),
        )?),
        GovernanceCommand::SubmitSetWithdrawalCooldown { admin, cooldown_ns } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_withdrawal_cooldown",
                args([
                    ("--caller", admin.as_str()),
                    ("--withdrawal_cooldown_ns", &cooldown_ns.to_string()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitSetIdleResyncCooldown { admin, cooldown_ns } => {
            invoke_response(stellar.invoke(
                governance,
                "submit_set_idle_resync_cooldown",
                args([
                    ("--caller", admin.as_str()),
                    ("--idle_resync_cooldown_ns", &cooldown_ns.to_string()),
                ]),
            )?)
        }
        GovernanceCommand::SubmitUpgrade { admin, wasm_hash } => invoke_response(stellar.invoke(
            governance,
            "submit_upgrade",
            args([
                ("--caller", admin.as_str()),
                ("--new_wasm_hash", wasm_hash.as_str()),
            ]),
        )?),
        GovernanceCommand::SubmitMigrate { admin } => invoke_response(stellar.invoke(
            governance,
            "submit_migrate",
            args([("--caller", admin.as_str())]),
        )?),
        GovernanceCommand::SubmitCancelMigration { admin } => invoke_response(stellar.invoke(
            governance,
            "submit_cancel_migration",
            args([("--caller", admin.as_str())]),
        )?),
        GovernanceCommand::Abdicate { admin, kind } => invoke_response(stellar.invoke(
            governance,
            "abdicate",
            args([("--caller", admin.as_str()), ("--kind", &kind.to_string())]),
        )?),
    }
}

pub(super) fn governance_plan(
    scope: impl Into<String>,
    network: &str,
    required_signers: Vec<String>,
    stellar_commands: Vec<String>,
) -> PlanResponse {
    let mut plan = PlanResponse::new(scope, network);
    plan.required_signers = required_signers;
    plan.stellar_commands = stellar_commands;
    plan.manifest_mutations
        .push("none; governance proposals are stored on-chain".to_string());
    plan
}

pub(super) fn governance_queue<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    governance: &str,
    kind: Option<&crate::types::GovernanceActionKindArg>,
) -> anyhow::Result<GovernanceQueueResponse> {
    let out = stellar.invoke_view(governance, "pending_ids", Vec::new())?;
    let ids = parse_u64s(&out.stdout);
    let mut proposals = Vec::new();
    let mut warnings = Vec::new();
    for proposal_id in ids {
        match inspect_governance_proposal(stellar, governance, proposal_id) {
            Ok(proposal) if proposal_matches_kind(&proposal, kind) => proposals.push(proposal),
            Ok(_) => {}
            Err(error) => warnings.push(format!("proposal {proposal_id}: {error}")),
        }
    }
    Ok(GovernanceQueueResponse {
        proposals,
        warnings,
    })
}

pub(super) fn inspect_governance_proposal<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    governance: &str,
    proposal_id: u64,
) -> anyhow::Result<GovernanceProposalView> {
    let out = stellar.invoke_view(
        governance,
        "pending",
        args([("--proposal_id", &proposal_id.to_string())]),
    )?;
    Ok(governance_proposal_view(proposal_id, out.stdout))
}

pub(super) fn governance_proposal_view(proposal_id: u64, raw: String) -> GovernanceProposalView {
    let valid_after_ns =
        parse_named_u64(&raw, "valid_after_ns").or_else(|| parse_named_u64(&raw, "valid_at_ns"));
    let now_ns = system_now_ns();
    let ready = valid_after_ns.map(|valid_after_ns| now_ns >= valid_after_ns);
    let eta_seconds = valid_after_ns.map(|valid_after_ns| {
        if now_ns >= valid_after_ns {
            0
        } else {
            i64::try_from((valid_after_ns - now_ns) / 1_000_000_000).unwrap_or(i64::MAX)
        }
    });
    GovernanceProposalView {
        proposal_id,
        action: summarize_governance_action(&raw),
        valid_after_ns,
        ready,
        eta_seconds,
        raw,
    }
}

pub(super) fn run_governance_accept_ready<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    governance: &str,
    admin: &AddressStr,
    kind: Option<&crate::types::GovernanceActionKindArg>,
    limit: Option<usize>,
) -> anyhow::Result<Response> {
    let queue = governance_queue(stellar, governance, kind)?;
    let mut accepted = Vec::new();
    let mut skipped = queue.warnings;
    for proposal in queue.proposals {
        if limit.is_some_and(|limit| accepted.len() >= limit) {
            skipped.push(format!("proposal {}: limit reached", proposal.proposal_id));
            continue;
        }
        match proposal.ready {
            Some(true) => {
                stellar.invoke(
                    governance,
                    "accept",
                    args([
                        ("--caller", admin.as_str()),
                        ("--proposal_id", &proposal.proposal_id.to_string()),
                    ]),
                )?;
                accepted.push(proposal.proposal_id);
            }
            Some(false) => skipped.push(format!(
                "proposal {}: not ready for {} seconds",
                proposal.proposal_id,
                proposal.eta_seconds.unwrap_or_default()
            )),
            None => skipped.push(format!(
                "proposal {}: readiness could not be decoded",
                proposal.proposal_id
            )),
        }
    }
    Ok(Response::GovernanceAcceptReady(
        GovernanceAcceptReadyResponse { accepted, skipped },
    ))
}

pub(super) fn run_governance_submit_and_wait<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    governance: &str,
    wait_args: &crate::cli::GovernanceSubmitAndWaitArgs,
) -> anyhow::Result<Response> {
    let (admin, proposal_id) = match &wait_args.command {
        GovernanceSubmitAndWaitCommand::Proposal { admin, proposal_id } => (admin, *proposal_id),
        GovernanceSubmitAndWaitCommand::SetSupplyQueue { admin, entries } => {
            let out = stellar.invoke(
                governance,
                "submit_set_supply_queue",
                vec![
                    "--caller".to_string(),
                    admin.to_string(),
                    "--entries".to_string(),
                    supply_queue_entries_json(entries)?,
                ],
            )?;
            (admin, parse_proposal_id(&out.stdout)?)
        }
        GovernanceSubmitAndWaitCommand::SetTimelock {
            admin,
            kind,
            timelock_ns,
        } => {
            let out = stellar.invoke(
                governance,
                "submit_set_timelock",
                args([
                    ("--caller", admin.as_str()),
                    ("--kind", &kind.to_string()),
                    ("--new_timelock_ns", &timelock_ns.to_string()),
                ]),
            )?;
            (admin, parse_proposal_id(&out.stdout)?)
        }
    };
    wait_for_governance_proposal(
        stellar,
        governance,
        admin,
        proposal_id,
        wait_args.poll_seconds,
        wait_args.max_wait_seconds,
    )
}

pub(super) fn wait_for_governance_proposal<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    governance: &str,
    admin: &AddressStr,
    proposal_id: u64,
    poll_seconds: u64,
    max_wait_seconds: u64,
) -> anyhow::Result<Response> {
    let started = SystemTime::now();
    loop {
        let proposal = inspect_governance_proposal(stellar, governance, proposal_id)?;
        if proposal.ready == Some(true) {
            stellar.invoke(
                governance,
                "accept",
                args([
                    ("--caller", admin.as_str()),
                    ("--proposal_id", &proposal_id.to_string()),
                ]),
            )?;
            return Ok(Response::message(format!(
                "accepted ready proposal {proposal_id}"
            )));
        }
        if max_wait_seconds == 0 {
            return Ok(Response::GovernanceExplain(proposal));
        }
        let elapsed = started.elapsed().unwrap_or_default().as_secs();
        if elapsed >= max_wait_seconds {
            return Ok(Response::GovernanceExplain(proposal));
        }
        let remaining = max_wait_seconds.saturating_sub(elapsed);
        thread::sleep(Duration::from_secs(poll_seconds.min(remaining).max(1)));
    }
}

pub(super) fn proposal_matches_kind(
    proposal: &GovernanceProposalView,
    kind: Option<&crate::types::GovernanceActionKindArg>,
) -> bool {
    let Some(kind) = kind else {
        return true;
    };
    governance_action_kind(&proposal.action) == Some(kind.0)
}

pub(super) fn summarize_governance_action(raw: &str) -> String {
    GOVERNANCE_ACTION_KINDS
        .iter()
        .find_map(|(action, _)| raw.contains(action).then_some(*action))
        .unwrap_or("unknown")
        .to_string()
}

fn governance_action_kind(action: &str) -> Option<GovernanceActionKind> {
    GOVERNANCE_ACTION_KINDS
        .iter()
        .find_map(|(candidate, kind)| (*candidate == action).then_some(*kind))
}

const GOVERNANCE_ACTION_KINDS: [(&str, GovernanceActionKind); 24] = [
    ("SetAdmin", GovernanceActionKind::Admin),
    ("SetCurator", GovernanceActionKind::Curator),
    ("SetGovernance", GovernanceActionKind::Governance),
    ("SetPaused", GovernanceActionKind::Pause),
    ("SetSupplyQueue", GovernanceActionKind::SupplyQueue),
    ("SetFees", GovernanceActionKind::Fees),
    ("SetRestrictions", GovernanceActionKind::Restrictions),
    ("SetSentinel", GovernanceActionKind::Sentinel),
    ("SetAllocators", GovernanceActionKind::Allocators),
    ("SetAllowedAdapters", GovernanceActionKind::AllowedAdapters),
    ("SetTimelock", GovernanceActionKind::TimelockConfig),
    ("SetCap", GovernanceActionKind::Cap),
    ("RemoveMarket", GovernanceActionKind::MarketRemoval),
    ("SetGroupCap", GovernanceActionKind::CapGroup),
    ("SetGroupRelCap", GovernanceActionKind::CapGroup),
    ("SetGroupMember", GovernanceActionKind::CapGroup),
    ("SetSkimRecipient", GovernanceActionKind::Skim),
    ("Skim", GovernanceActionKind::Skim),
    ("Upgrade", GovernanceActionKind::Upgrade),
    ("CancelMigration", GovernanceActionKind::CancelMigration),
    ("Migrate", GovernanceActionKind::Migrate),
    (
        "SetWithdrawalCooldown",
        GovernanceActionKind::WithdrawalCooldown,
    ),
    (
        "SetIdleResyncCooldown",
        GovernanceActionKind::IdleResyncCooldown,
    ),
    ("Other", GovernanceActionKind::Other),
];

pub(super) fn parse_named_u64(raw: &str, name: &str) -> Option<u64> {
    let start = raw.find(name)? + name.len();
    raw[start..]
        .split(|c: char| !c.is_ascii_digit())
        .find(|part| !part.is_empty())?
        .parse()
        .ok()
}

pub(super) fn parse_u64s(raw: &str) -> Vec<u64> {
    raw.split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

pub(super) fn system_now_ns() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

pub(super) fn submit_and_maybe_accept<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &Manifest,
    admin: &str,
    submit_method: &str,
    submit_args: Vec<String>,
    auto_accept: bool,
) -> anyhow::Result<Response> {
    let governance = required_contract(manifest, "governance")?;
    let out = stellar.invoke(governance, submit_method, submit_args)?;
    if auto_accept {
        let proposal_id = parse_proposal_id(&out.stdout)?;
        stellar.invoke(
            governance,
            "accept",
            args([
                ("--caller", admin),
                ("--proposal_id", &proposal_id.to_string()),
            ]),
        )?;
        Ok(Response::message(format!(
            "submitted and accepted proposal {proposal_id}"
        )))
    } else {
        invoke_response(out)
    }
}
