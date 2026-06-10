use near_sdk::serde_json::json;
use near_sdk::AccountId;
use templar_common::oracle::redstone::Role;
use templar_tools_common::near;

use crate::CliContext;

use super::super::CliRole;

#[derive(clap::Args, Debug)]
pub struct RoleList {
    /// RedStone adapter contract account ID
    #[arg(long)]
    pub adapter_id: AccountId,
    /// Role to list members of
    #[arg(long)]
    pub role: CliRole,
}

impl RoleList {
    #[tracing::instrument(skip_all, name = "redstone_adapter_role_list", fields(adapter_id = %self.adapter_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let role: Role = self.role.clone().into();

        let members: Vec<AccountId> = near::view(
            &ctx.near,
            &self.adapter_id,
            "list_role",
            json!({ "role": role }),
        )
        .await?;

        if members.is_empty() {
            println!("No accounts with this role");
            return Ok(());
        }

        for member in &members {
            println!("{member}");
        }

        Ok(())
    }
}
