use std::io::Write;

use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_tools_common::near;

use crate::{
    util::{OutputArgs, OutputStyle},
    CliContext,
};

#[derive(serde::Serialize)]
struct VersionListOutput {
    versions: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct ListVersions {
    #[arg(long)]
    pub registry_id: AccountId,
    #[command(flatten)]
    pub output: OutputArgs,
}

impl ListVersions {
    #[tracing::instrument(skip_all, name = "list_versions", fields(registry_id = %self.registry_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let versions: Vec<String> =
            near::view(&ctx.near, &self.registry_id, "list_versions", json!({})).await?;

        let output = VersionListOutput { versions };
        self.output.print(&output)?;

        tracing::info!(count = output.versions.len(), "Listed versions");
        Ok(())
    }
}

impl OutputStyle for VersionListOutput {
    fn fmt_human(&self, out: &mut dyn Write) -> anyhow::Result<()> {
        for version in &self.versions {
            writeln!(out, "{version}")?;
        }

        Ok(())
    }
}
