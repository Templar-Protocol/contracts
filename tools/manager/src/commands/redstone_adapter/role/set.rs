use near_sdk::serde_json::json;
use near_sdk::{AccountId, NearToken};
use templar_common::oracle::redstone::Role;
use templar_tools_common::near::Function;

use crate::util::SignerArgs;
use crate::CliContext;

use super::super::CliRole;

#[derive(clap::Args, Debug)]
pub struct RoleSet {
    #[command(flatten)]
    pub signer: SignerArgs,
    /// RedStone adapter contract account ID
    #[arg(long)]
    pub adapter_id: AccountId,
    /// Account to grant or revoke the role for
    #[arg(long)]
    pub target_account_id: AccountId,
    /// Role to set
    #[arg(long)]
    pub role: CliRole,
    /// Revoke the role instead of granting it
    #[arg(long)]
    pub revoke: bool,
}

impl RoleSet {
    #[tracing::instrument(skip_all, name = "redstone_adapter_role_set", fields(adapter_id = %self.adapter_id, target = %self.target_account_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let role: Role = self.role.clone().into();
        let set = !self.revoke;

        let action = if set { "Granting" } else { "Revoking" };
        tracing::info!(%action, role = ?self.role, account = %self.target_account_id, "Setting role");

        let signer = self.signer.signer();
        ctx.batch(&signer, &self.adapter_id)
            .call(
                Function::new("set_role")
                    .args_json(json!({
                        "account_id": self.target_account_id,
                        "role": role,
                        "set": set,
                    }))?
                    .deposit(NearToken::from_yoctonear(1))
                    .max_gas(),
            )
            .transact()
            .await?;

        tracing::info!("Role updated");
        Ok(())
    }
}
