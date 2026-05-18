use std::io::Write;

use console::style;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::{governance::Proposal, Nanoseconds};
use templar_proxy_oracle_near_common::governance::Operation;
use templar_tools_common::near;

use crate::{
    util::{OutputArgs, OutputStyle},
    CliContext,
};

#[derive(serde::Serialize)]
struct ProposalListItem {
    id: u32,
    proposal: Proposal<Operation>,
    executable: bool,
}

#[derive(serde::Serialize)]
struct ProposalListOutput {
    proposals: Vec<ProposalListItem>,
}

#[derive(clap::Args, Debug)]
pub struct ListProposals {
    #[arg(long)]
    pub oracle_id: AccountId,
    #[command(flatten)]
    pub output: OutputArgs,
}

impl ListProposals {
    #[tracing::instrument(skip_all, name = "governance_list", fields(oracle_id = %self.oracle_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let ids: Vec<u32> = near::view(&ctx.near, &self.oracle_id, "gov_list", json!({})).await?;

        #[allow(clippy::unwrap_used, clippy::cast_possible_truncation)]
        let now = Nanoseconds::from_ms(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        );

        let mut proposals = Vec::new();

        for id in &ids {
            let proposal: Option<Proposal<Operation>> =
                near::view(&ctx.near, &self.oracle_id, "gov_get", json!({ "id": id })).await?;

            let Some(proposal) = proposal else {
                continue;
            };

            proposals.push(ProposalListItem {
                id: *id,
                executable: proposal.can_execute(now),
                proposal,
            });
        }

        let output = ProposalListOutput { proposals };
        self.output.print(&output)?;

        tracing::info!(count = output.proposals.len(), "Listed proposals");
        Ok(())
    }
}

impl OutputStyle for ProposalListOutput {
    fn fmt_human(&self, out: &mut dyn Write) -> anyhow::Result<()> {
        if self.proposals.is_empty() {
            writeln!(out, "{}", style("No active proposals.").dim())?;
            return Ok(());
        }

        let ttl = self.proposals[0].proposal.ttl;
        writeln!(
            out,
            "{}",
            style(format!("TTL: {} ({}s)", ttl, ttl.as_secs())).dim()
        )?;
        writeln!(out)?;

        writeln!(
            out,
            "  {:>4}  {:<14}  {:<44}  {:>10}",
            style("ID").bold(),
            style("Operation").bold(),
            style("Created By").bold(),
            style("Status").bold(),
        )?;

        for item in &self.proposals {
            let operation_name = match &item.proposal.operation {
                Operation::SetProxy { .. } => "SetProxy",
                Operation::SetActionTtl { .. } => "SetActionTtl",
                Operation::SetCircuitBreakerRole { .. } => "SetCircuitBreakerRole",
                Operation::ConfigureCircuitBreakers { .. } => "ConfigureCircuitBreakers",
                Operation::SetCircuitBreakerManualTrip { .. } => "SetCircuitBreakerManualTrip",
                Operation::AddCircuitBreaker { .. } => "AddCircuitBreaker",
                Operation::RemoveCircuitBreaker { .. } => "RemoveCircuitBreaker",
                Operation::UpdateCircuitBreaker { .. } => "UpdateCircuitBreaker",
            };

            let status = if item.executable {
                style("ready").green()
            } else {
                style("pending").yellow()
            };

            writeln!(
                out,
                "  {:>4}  {:<14}  {:<44}  {:>10}",
                style(item.id).bold(),
                operation_name,
                item.proposal.created_by,
                status,
            )?;
        }

        Ok(())
    }
}
