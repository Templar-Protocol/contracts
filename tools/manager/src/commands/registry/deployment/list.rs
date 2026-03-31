use crate::near;
use console::style;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::registry::Deployment;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct ListDeployments {
    #[arg(long)]
    pub registry_id: AccountId,
}

impl ListDeployments {
    #[tracing::instrument(skip_all, name = "list_deployments", fields(registry_id = %self.registry_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let deployments: Vec<AccountId> = ctx
            .near
            .view(&self.registry_id, "list_deployments")
            .args_json(json!({}))
            .await?
            .json()?;

        if deployments.is_empty() {
            println!("{}", style("No deployments found.").dim());
            return Ok(());
        }

        let account_width = deployments
            .iter()
            .map(|d| d.as_str().len())
            .max()
            .unwrap_or(0);

        for deployment in &deployments {
            let info: Option<Deployment> = ctx
                .near
                .view(&self.registry_id, "get_deployment")
                .args_json(json!({ "account_id": deployment }))
                .await?
                .json()?;

            let version_key = info.as_ref().map_or("unknown", |d| d.version_key.as_str());

            let exists = near::account_exists(&ctx.near, deployment).await?;
            let status = if exists {
                style("exists").green()
            } else {
                style("deleted").red()
            };

            println!(
                "  {:<width$}  {:>10}  {}",
                style(deployment).bold(),
                status,
                style(version_key).dim(),
                width = account_width,
            );
        }

        tracing::info!(count = deployments.len(), "Listed deployments");
        Ok(())
    }
}
