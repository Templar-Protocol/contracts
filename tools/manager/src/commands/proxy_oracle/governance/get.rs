use console::style;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::{
    oracle::proxy::governance::{Operation, Proposal},
    time::Nanoseconds,
};

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct GetProposal {
    #[arg(long)]
    pub oracle_id: AccountId,
    /// Proposal ID
    #[arg(long)]
    pub id: u32,
}

impl GetProposal {
    #[tracing::instrument(skip_all, name = "governance_get", fields(oracle_id = %self.oracle_id, id = self.id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let ttl: Nanoseconds = ctx.near.view(&self.oracle_id, "gov_ttl_ns").await?.json()?;

        let proposal: Option<Proposal<Operation>> = ctx
            .near
            .view(&self.oracle_id, "gov_get")
            .args_json(json!({ "id": self.id }))
            .await?
            .json()?;

        let Some(proposal) = proposal else {
            println!("Proposal {} not found", self.id);
            return Ok(());
        };

        #[allow(clippy::unwrap_used, clippy::cast_possible_truncation)]
        let now = Nanoseconds::from_ms(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        );

        let executable = proposal.can_execute(now);

        println!("{}: {}", style("Proposal").bold(), self.id);
        println!("{}: {}", style("Created by").bold(), proposal.created_by);
        println!("{}: {}", style("Created at").bold(), proposal.created_at);
        println!("{}: {}s", style("TTL").bold(), ttl.as_secs());

        if executable {
            println!("{}: {}", style("Status").bold(), style("ready").green());
        } else {
            let ready_at = proposal.created_at.saturating_add(ttl);
            let remaining = ready_at.saturating_sub(now);
            println!(
                "{}: {} ({}s remaining)",
                style("Status").bold(),
                style("pending").yellow(),
                remaining.as_secs(),
            );
        }

        println!();
        println!("{}:", style("Operation").bold());
        match &proposal.operation {
            Operation::SetProxy { id, proxy } => {
                println!("  SetProxy");
                println!("    price_id: {id}");
                match proxy {
                    Some(proxy) => {
                        println!(
                            "    proxy: {} entries, aggregator={:?}",
                            proxy.entries.len(),
                            proxy.aggregator.method,
                        );
                    }
                    None => {
                        println!("    proxy: {}", style("remove").red());
                    }
                }
            }
            Operation::SetActionTtl { new_ttl } => {
                println!("  SetActionTtl");
                println!("    new_ttl: {} ({}s)", new_ttl, new_ttl.as_secs());
            }
        }

        Ok(())
    }
}
