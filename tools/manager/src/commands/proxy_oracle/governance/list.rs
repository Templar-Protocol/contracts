use console::style;
use near_sdk::json_types::U64;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::oracle::proxy::governance::{Operation, Proposal};

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct ListProposals {
    #[arg(long)]
    pub oracle_id: AccountId,
}

impl ListProposals {
    #[tracing::instrument(skip_all, name = "governance_list", fields(oracle_id = %self.oracle_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let ttl_ms: U64 = ctx.near.view(&self.oracle_id, "gov_ttl_ms").await?.json()?;

        let ids: Vec<u32> = ctx
            .near
            .view(&self.oracle_id, "gov_list")
            .args_json(json!({}))
            .await?
            .json()?;

        if ids.is_empty() {
            println!("{}", style("No active proposals.").dim());
            return Ok(());
        }

        println!("{}", style(format!("TTL: {}ms", ttl_ms.0)).dim());
        println!();

        #[allow(clippy::unwrap_used, clippy::cast_possible_truncation)]
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        println!(
            "  {:>4}  {:<12}  {:<44}  {:>10}",
            style("ID").bold(),
            style("Operation").bold(),
            style("Created By").bold(),
            style("Status").bold(),
        );

        let mut count = 0;

        for id in &ids {
            let proposal: Option<Proposal<Operation>> = ctx
                .near
                .view(&self.oracle_id, "gov_get")
                .args_json(json!({ "id": id }))
                .await?
                .json()?;

            let Some(proposal) = proposal else {
                continue;
            };

            let operation_name = match &proposal.operation {
                Operation::SetProxy { .. } => "SetProxy",
                Operation::SetActionTtl { .. } => "SetActionTtl",
            };

            let executable = proposal.can_execute(now_ms, ttl_ms.0);
            let status = if executable {
                style("ready").green()
            } else {
                style("pending").yellow()
            };

            println!(
                "  {:>4}  {:<12}  {:<44}  {:>10}",
                style(id).bold(),
                operation_name,
                proposal.created_by,
                status,
            );

            count += 1;
        }

        tracing::info!(count, "Listed proposals");
        Ok(())
    }
}
