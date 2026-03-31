use anyhow::Context;
use near_fetch::ops::Function;
use near_sdk::serde_json::json;
use templar_tools_common::version;

use crate::commands::{FixedContractWasm, SignerArgs};

const REGISTRY_PACKAGE: &str = "templar-registry-contract";

#[derive(clap::Args, Debug)]
pub struct DeployRegistry {
    #[command(flatten)]
    pub signer: SignerArgs,
    #[command(flatten)]
    pub contract: FixedContractWasm,
    #[arg(long)]
    pub no_init: bool,
}

impl DeployRegistry {
    #[tracing::instrument(skip_all, name = "deploy_registry", fields(account_id = %self.signer.account_id))]
    pub async fn run(self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let loaded_contract = self
            .contract
            .load_contract::<version::Registry>(ctx, REGISTRY_PACKAGE)?;
        tracing::info!(version = %loaded_contract.version, "Deploying registry");

        let signer = self.signer.signer();
        if self.no_init {
            ctx.batch(&signer, &self.signer.account_id)
                .deploy(&loaded_contract.wasm_bytes)
                .transact()
                .await
                .context("deploy registry without init")?;
        } else {
            ctx.batch(&signer, &self.signer.account_id)
                .deploy(&loaded_contract.wasm_bytes)
                .call(Function::new("new").args_json(json!({})).max_gas())
                .transact()
                .await
                .context("deploy registry with init")?;
        }

        tracing::info!("Registry deployed successfully");
        Ok(())
    }
}
