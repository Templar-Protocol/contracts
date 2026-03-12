use console::style;
use near_sdk::json_types::U64;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::oracle::proxy::governance::{Operation, Proposal};

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct GetProposal {
    #[arg(long)]
    oracle_id: AccountId,
    /// Proposal ID
    #[arg(long)]
    id: u32,
}

impl GetProposal {
    #[tracing::instrument(skip_all, name = "governance_get", fields(oracle_id = %self.oracle_id, id = self.id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let ttl_ms: U64 = ctx.near.view(&self.oracle_id, "gov_ttl_ms").await?.json()?;

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
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let executable = proposal.can_execute(now_ms, ttl_ms.0);

        println!("{}: {}", style("Proposal").bold(), self.id);
        println!("{}: {}", style("Created by").bold(), proposal.created_by);
        println!(
            "{}: {}ms",
            style("Created at").bold(),
            proposal.created_at_ms.0
        );
        println!("{}: {}ms", style("TTL").bold(), ttl_ms.0);

        if executable {
            println!("{}: {}", style("Status").bold(), style("ready").green());
        } else {
            let remaining = (proposal.created_at_ms.0 + ttl_ms.0).saturating_sub(now_ms);
            println!(
                "{}: {} ({}ms remaining)",
                style("Status").bold(),
                style("pending").yellow(),
                remaining,
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
            Operation::SetActionTtl { new_ttl_ms } => {
                println!("  SetActionTtl");
                println!("    new_ttl_ms: {}", new_ttl_ms.0);
            }
        }

        Ok(())
    }
}
