use std::io::Write;

use crate::near;
use console::style;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::registry::Deployment;

use crate::{
    util::{OutputArgs, OutputStyle},
    CliContext,
};

#[derive(serde::Serialize)]
struct DeploymentListEntry {
    account_id: AccountId,
    info: Option<Deployment>,
    exists: bool,
}

#[derive(serde::Serialize)]
struct DeploymentListOutput {
    deployments: Vec<DeploymentListEntry>,
}

#[derive(clap::Args, Debug)]
pub struct ListDeployments {
    #[arg(long)]
    pub registry_id: AccountId,
    #[command(flatten)]
    pub output: OutputArgs,
}

impl ListDeployments {
    #[tracing::instrument(skip_all, name = "list_deployments", fields(registry_id = %self.registry_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let deployments: Vec<AccountId> =
            near::view(&ctx.near, &self.registry_id, "list_deployments", json!({})).await?;

        let mut entries = Vec::with_capacity(deployments.len());

        for deployment in &deployments {
            let info: Option<Deployment> = near::view(
                &ctx.near,
                &self.registry_id,
                "get_deployment",
                json!({ "account_id": deployment }),
            )
            .await?;

            let exists = near::account_exists(&ctx.near, deployment).await?;
            entries.push(DeploymentListEntry {
                account_id: deployment.clone(),
                info,
                exists,
            });
        }

        let output = DeploymentListOutput {
            deployments: entries,
        };
        self.output.print(&output)?;

        tracing::info!(count = output.deployments.len(), "Listed deployments");
        Ok(())
    }
}

impl OutputStyle for DeploymentListOutput {
    fn fmt_human(&self, out: &mut dyn Write) -> anyhow::Result<()> {
        if self.deployments.is_empty() {
            writeln!(out, "{}", style("No deployments found.").dim())?;
            return Ok(());
        }

        let account_width = self
            .deployments
            .iter()
            .map(|d| d.account_id.as_str().len())
            .max()
            .unwrap_or(0);

        for entry in &self.deployments {
            let version_key = entry
                .info
                .as_ref()
                .map_or("unknown", |d| d.version_key.as_str());

            let status = if entry.exists {
                style("exists").green()
            } else {
                style("deleted").red()
            };

            writeln!(
                out,
                "  {:<width$}  {:>10}  {}",
                style(&entry.account_id).bold(),
                status,
                style(version_key).dim(),
                width = account_width,
            )?;
        }

        Ok(())
    }
}
