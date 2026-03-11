use anyhow::Context;
use near_fetch::ops::Function;
use near_sdk::serde_json::json;

use crate::commands::ContractWasm;

const REGISTRY_PACKAGE: &str = "templar_registry_contract";
const REGISTRY_CONTRACT_DIR: &str = "contract/registry";

#[derive(clap::Args, Debug)]
pub struct DeployRegistry {
    #[command(flatten)]
    signer: super::SignerArgs,
    #[arg(long)]
    no_build: bool,
    #[arg(long)]
    no_init: bool,
}

impl DeployRegistry {
    #[tracing::instrument(skip(context))]
    pub async fn run(self, context: &crate::CliContext) -> anyhow::Result<()> {
        let wasm = ContractWasm::new(REGISTRY_PACKAGE)
            .no_build(self.no_build)
            .wasm(context)?;

        if self.no_init {
            context
                .near
                .batch(&self.signer.signer(), &self.signer.account_id)
                .deploy(&wasm)
                .transact()
                .await
                .context("deploy registry without init")?;
        } else {
            context
                .near
                .batch(&self.signer.signer(), &self.signer.account_id)
                .deploy(&wasm)
                .call(Function::new("new").args_json(json!({})).max_gas())
                .transact()
                .await
                .context("deploy registry with init")?;
        }

        tracing::info!("Registry deployed successfully");
        Ok(())
    }
}
