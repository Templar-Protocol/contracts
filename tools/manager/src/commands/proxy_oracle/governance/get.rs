use std::io::Write;

use console::style;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::Nanoseconds;
use templar_proxy_oracle_near_governance_common::{Operation, Proposal};
use templar_tools_common::near;

use crate::{
    util::{OutputArgs, OutputStyle},
    CliContext,
};

#[derive(clap::Args, Debug)]
pub struct GetProposal {
    #[arg(long)]
    pub oracle_id: AccountId,
    /// Proposal ID
    #[arg(long)]
    pub id: u32,
    #[command(flatten)]
    pub output: OutputArgs,
}

impl GetProposal {
    #[tracing::instrument(skip_all, name = "governance_get", fields(oracle_id = %self.oracle_id, id = self.id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let proposal: Option<Proposal<Operation>> = near::view(
            &ctx.near,
            &self.oracle_id,
            "get_proposal",
            json!({ "id": self.id }),
        )
        .await?;

        self.output.print_optional(proposal.as_ref(), |out| {
            writeln!(out, "Proposal {} not found", self.id)?;
            Ok(())
        })
    }
}

impl OutputStyle for Proposal<Operation> {
    #[allow(clippy::too_many_lines)]
    fn fmt_human(&self, out: &mut dyn Write) -> anyhow::Result<()> {
        #[allow(clippy::unwrap_used, clippy::cast_possible_truncation)]
        let now = Nanoseconds::from_ms(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        );

        let executable = self.can_execute(now);

        writeln!(out, "{}: {}", style("Created by").bold(), self.created_by)?;
        writeln!(out, "{}: {}", style("Created at").bold(), self.created_at)?;
        writeln!(out, "{}: {}s", style("TTL").bold(), self.ttl.as_secs())?;

        if executable {
            writeln!(
                out,
                "{}: {}",
                style("Status").bold(),
                style("ready").green(),
            )?;
        } else {
            let ready_at = self.created_at.saturating_add(self.ttl);
            let remaining = ready_at.saturating_sub(now);
            writeln!(
                out,
                "{}: {} ({}s remaining)",
                style("Status").bold(),
                style("pending").yellow(),
                remaining.as_secs(),
            )?;
        }

        writeln!(out)?;
        writeln!(out, "{}:", style("Operation").bold())?;

        match &self.operation {
            Operation::SetProxy { id, proxy } => {
                writeln!(out, "  SetProxy")?;
                writeln!(out, "    price_id: {id}")?;

                match proxy {
                    Some(proxy) => {
                        let aggregator_name = proxy.aggregator.name();
                        let entry_count = proxy.sources().len();

                        writeln!(
                            out,
                            "    proxy: {entry_count} entries, aggregator={aggregator_name}",
                        )?;
                    }
                    None => {
                        writeln!(out, "    proxy: {}", style("remove").red())?;
                    }
                }
            }
            Operation::ConfigureCircuitBreakers { id, config } => {
                writeln!(out, "  ConfigureCircuitBreakers")?;
                writeln!(out, "    price_id: {id}")?;
                writeln!(out, "    sample_interval_ns: {}", config.sample_interval_ns)?;
                writeln!(out, "    history_len: {}", config.history_len)?;
            }
            Operation::AddCircuitBreaker {
                id,
                breaker_id,
                breaker,
            } => {
                writeln!(out, "  AddCircuitBreaker")?;
                writeln!(out, "    price_id: {id}")?;
                writeln!(out, "    breaker_id: {breaker_id}")?;
                writeln!(out, "    breaker: {breaker:?}")?;
            }
            Operation::RemoveCircuitBreaker { id, breaker_id } => {
                writeln!(out, "  RemoveCircuitBreaker")?;
                writeln!(out, "    price_id: {id}")?;
                writeln!(out, "    breaker_id: {breaker_id}")?;
            }
            Operation::SetManualTrip {
                id,
                is_manually_tripped,
                metadata,
            } => {
                writeln!(out, "  SetManualTrip")?;
                writeln!(out, "    price_id: {id}")?;
                writeln!(out, "    is_manually_tripped: {is_manually_tripped}")?;
                writeln!(out, "    metadata: {metadata:?}")?;
            }
            Operation::Rearm {
                id,
                breaker_id,
                armed_after_ns,
                accepted_history_source,
            } => {
                writeln!(out, "  Rearm")?;
                writeln!(out, "    price_id: {id}")?;
                writeln!(out, "    breaker_id: {breaker_id}")?;
                writeln!(out, "    armed_after_ns: {armed_after_ns}")?;
                writeln!(
                    out,
                    "    accepted_history_source: {accepted_history_source:?}"
                )?;
            }
            Operation::SetEnforced {
                id,
                breaker_id,
                is_enforced,
            } => {
                writeln!(out, "  SetEnforced")?;
                writeln!(out, "    price_id: {id}")?;
                writeln!(out, "    breaker_id: {breaker_id}")?;
                writeln!(out, "    is_enforced: {is_enforced}")?;
            }
            Operation::SetActionTtl { kind, new_ttl } => {
                writeln!(out, "  SetActionTtl")?;
                writeln!(out, "    kind: {kind:?}")?;
                writeln!(out, "    new_ttl: {new_ttl}")?;
            }
            Operation::SetRole {
                account_id,
                role,
                set,
            } => {
                writeln!(out, "  SetRole")?;
                writeln!(out, "    account_id: {account_id}")?;
                writeln!(out, "    role: {role:?}")?;
                writeln!(out, "    set: {set}")?;
            }
            Operation::AdminUpgrade { code, migrate_args } => {
                writeln!(out, "  AdminUpgrade")?;
                writeln!(out, "    code: {} bytes", code.0.len())?;
                writeln!(out, "    migrate_args: {} bytes", migrate_args.0.len())?;
            }
            Operation::AdminFunctionCall {
                method_name,
                args,
                attached_deposit,
                gas,
            } => {
                writeln!(out, "  AdminFunctionCall")?;
                writeln!(out, "    method_name: {method_name}")?;
                writeln!(out, "    args: {} bytes", args.0.len())?;
                writeln!(
                    out,
                    "    attached_deposit: {} yoctoNEAR",
                    attached_deposit.0
                )?;
                writeln!(out, "    gas: {}", gas.as_gas())?;
            }
        }

        Ok(())
    }
}
