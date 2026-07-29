//! Human-readable and JSON response rendering.

use crate::cli::Cli;

use super::{error::OutputEnvelope, GovernanceProposalView, PlanContract, Response};

#[allow(
    clippy::too_many_lines,
    reason = "single response printer keeps CLI human output routing explicit"
)]
pub(in crate::commands) fn print_response(response: &Response, cli: &Cli) -> anyhow::Result<()> {
    if cli.json || cli.json_lines {
        println!(
            "{}",
            serde_json::to_string(&OutputEnvelope::success(cli, response))?
        );
        return Ok(());
    }
    match response {
        Response::Message { message } => println!("{message}"),
        Response::Command { stdout, stderr } => {
            if !stdout.is_empty() {
                println!("{stdout}");
            }
            if !stderr.is_empty() {
                eprintln!("{stderr}");
            }
        }
        Response::Status(status) => {
            println!("Network: {}", status.network);
            print_optional("Vault", status.vault.as_ref());
            print_optional("Share Token", status.share_token.as_ref());
            print_optional("Governance", status.governance.as_ref());
            print_optional("Asset Token", status.asset_token.as_ref());
            print_optional("ERC-4626 Proxy", status.proxy_4626.as_ref());
            print_optional("Curator Proxy", status.curator_proxy.as_ref());
            if status.blend_adapters.is_empty() {
                println!("Blend Adapters: not deployed");
            } else {
                for adapter in &status.blend_adapters {
                    println!(
                        "Blend Adapter {}: {}{}",
                        adapter.key,
                        adapter.contract_id,
                        adapter
                            .pool
                            .as_ref()
                            .map_or_else(String::new, |pool| format!(" (pool {pool})"))
                    );
                }
            }
            if status.custodial_adapters.is_empty() {
                println!("Custodial Adapters: not deployed");
            } else {
                for adapter in &status.custodial_adapters {
                    println!(
                        "Custodial Adapter {}: {}{}{}",
                        adapter.key,
                        adapter.contract_id,
                        adapter
                            .custodian
                            .as_ref()
                            .map_or_else(String::new, |custodian| {
                                format!(" (custodian {custodian})")
                            }),
                        adapter
                            .asset
                            .as_ref()
                            .map_or_else(String::new, |asset| format!(" (asset {asset})"))
                    );
                }
            }
        }
        Response::Env(values) => {
            for (key, value) in values {
                println!("{key}={value}");
            }
        }
        Response::ExtendTtl(result) => {
            if result.extended.is_empty() {
                println!("Extended TTL: none");
            } else {
                println!("Extended TTL: {}", result.extended.join(", "));
            }
            if !result.skipped.is_empty() {
                println!("Skipped: {}", result.skipped.join(", "));
            }
        }
        Response::Reconcile(result) => {
            println!("Safe to resume: {}", result.safe_to_resume);
            println!("Drift detected: {}", result.drift_detected);
            println!("Components:");
            for component in &result.components {
                println!(
                    "  - {}: {}{}",
                    component.key,
                    component.status.as_label(),
                    component
                        .contract_id
                        .as_ref()
                        .map(|id| format!(" ({id})"))
                        .unwrap_or_default()
                );
                for warning in &component.warnings {
                    println!("    warning: {warning}");
                }
            }
            if !result.repair_actions.is_empty() {
                println!("Repair plan:");
                for action in &result.repair_actions {
                    println!("  - {action}");
                }
            }
            if !result.safe_next_steps.is_empty() {
                println!("Next steps:");
                for step in &result.safe_next_steps {
                    println!("  - {step}");
                }
            }
        }
        Response::Doctor(result) => {
            println!(
                "Doctor: {}",
                if result.ok {
                    "ready"
                } else {
                    "action required"
                }
            );
            for check in &result.checks {
                println!(
                    "[{}] {}: {}",
                    check.status.as_label(),
                    check.name,
                    check.message
                );
            }
        }
        Response::Plan(plan) => {
            println!("Plan: {} ({})", plan.scope, plan.network);
            if !plan.required_signers.is_empty() {
                println!("Required signers: {}", plan.required_signers.join(", "));
            }
            print_plan_contracts("Reuse", &plan.contracts_to_reuse);
            print_plan_contracts("Deploy", &plan.contracts_to_deploy);
            if !plan.wasm.is_empty() {
                println!("WASM:");
                for wasm in &plan.wasm {
                    println!("  - {}: {}", wasm.key, wasm.action);
                    if let Some(hash) = &wasm.local_hash {
                        println!("    local hash: {hash}");
                    }
                    if let Some(hash) = &wasm.recorded_remote_hash {
                        println!("    recorded remote hash: {hash}");
                    }
                }
            }
            if !plan.manifest_mutations.is_empty() {
                println!("Manifest mutations:");
                for mutation in &plan.manifest_mutations {
                    println!("  - {mutation}");
                }
            }
            if !plan.stellar_commands.is_empty() {
                println!("Stellar commands:");
                for command in &plan.stellar_commands {
                    println!("  - {command}");
                }
            }
            if !plan.warnings.is_empty() {
                println!("Warnings:");
                for warning in &plan.warnings {
                    println!("  - {warning}");
                }
            }
        }
        Response::GovernanceQueue(queue) => {
            if queue.proposals.is_empty() {
                println!("Governance queue: no matching pending proposals");
            } else {
                println!("Governance queue:");
                for proposal in &queue.proposals {
                    print_governance_proposal(proposal);
                }
            }
            for warning in &queue.warnings {
                println!("Warning: {warning}");
            }
        }
        Response::GovernanceExplain(proposal) => {
            print_governance_proposal(proposal);
            println!("Raw: {}", proposal.raw);
        }
        Response::GovernanceAcceptReady(result) => {
            if result.accepted.is_empty() {
                println!("Accepted proposals: none");
            } else {
                println!("Accepted proposals: {:?}", result.accepted);
            }
            if !result.skipped.is_empty() {
                println!("Skipped:");
                for skipped in &result.skipped {
                    println!("  - {skipped}");
                }
            }
        }
    }
    Ok(())
}

fn print_governance_proposal(proposal: &GovernanceProposalView) {
    println!(
        "  - #{} {} ready={} eta_seconds={}",
        proposal.proposal_id,
        proposal.action,
        proposal
            .ready
            .map_or_else(|| "unknown".to_string(), |ready| ready.to_string()),
        proposal
            .eta_seconds
            .map_or_else(|| "unknown".to_string(), |eta| eta.to_string())
    );
}

fn print_plan_contracts(label: &str, contracts: &[PlanContract]) {
    if contracts.is_empty() {
        return;
    }
    println!("{label}:");
    for contract in contracts {
        println!(
            "  - {}{}: {}",
            contract.key,
            contract
                .contract_id
                .as_ref()
                .map_or_else(String::new, |id| format!(" ({id})")),
            contract.reason
        );
    }
}

fn print_optional(label: &str, value: Option<&String>) {
    println!(
        "{}: {}",
        label,
        value.map_or("not deployed", String::as_str)
    );
}
