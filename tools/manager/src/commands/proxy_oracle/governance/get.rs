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
            "gov_get",
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
            Operation::SetActionTtl { new_ttl } => {
                writeln!(out, "  SetActionTtl")?;
                writeln!(out, "    new_ttl: {} ({}s)", new_ttl, new_ttl.as_secs())?;
            }
            Operation::SetCircuitBreakerSetConfig { id, config } => {
                writeln!(out, "  SetCircuitBreakerSetConfig")?;
                writeln!(out, "    price_id: {id}")?;
                writeln!(out, "    sample_interval_ns: {}", config.sample_interval_ns)?;
                writeln!(out, "    history_len: {}", config.history_len)?;
            }
            Operation::SetCircuitBreakerSetManualTrip {
                id,
                is_manually_tripped,
            } => {
                writeln!(out, "  SetCircuitBreakerSetManualTrip")?;
                writeln!(out, "    price_id: {id}")?;
                writeln!(out, "    is_manually_tripped: {is_manually_tripped}")?;
            }
            Operation::AddCircuitBreaker { id, order, breaker } => {
                writeln!(out, "  AddCircuitBreaker")?;
                writeln!(out, "    price_id: {id}")?;
                writeln!(out, "    order: {order}")?;
                writeln!(out, "    breaker: {breaker:?}")?;
            }
            Operation::RemoveCircuitBreaker { id, breaker_id } => {
                writeln!(out, "  RemoveCircuitBreaker")?;
                writeln!(out, "    price_id: {id}")?;
                writeln!(out, "    breaker_id: {breaker_id}")?;
            }
            Operation::SetCircuitBreakerStatus {
                id,
                breaker_id,
                status,
            } => {
                writeln!(out, "  SetCircuitBreakerStatus")?;
                writeln!(out, "    price_id: {id}")?;
                writeln!(out, "    breaker_id: {breaker_id}")?;
                writeln!(out, "    status: {status:?}")?;
            }
        }

        Ok(())
    }
}
