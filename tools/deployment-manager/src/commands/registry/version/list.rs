use near_sdk::serde_json::json;
use near_sdk::AccountId;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct ListVersions {
    #[arg(long)]
    registry_id: AccountId,
}

impl ListVersions {
    #[tracing::instrument(skip_all, name = "list_versions", fields(registry_id = %self.registry_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let versions: Vec<String> = ctx
            .near
            .view(&self.registry_id, "list_versions")
            .args_json(json!({}))
            .await?
            .json()?;

        for version in &versions {
            println!("{version}");
        }

        tracing::info!(count = versions.len(), "Listed versions");
        Ok(())
    }
}
