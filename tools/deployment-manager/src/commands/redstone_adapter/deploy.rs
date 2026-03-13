use anyhow::Context;
use near_fetch::ops::Function;

use crate::commands::{FixedContractWasm, SignerArgs};

use super::create::ConfigSource;

const REDSTONE_ADAPTER_PACKAGE: &str = "templar-redstone-adapter-contract";

#[derive(clap::Args, Debug)]
pub struct DeployRedStoneAdapter {
    #[command(flatten)]
    signer: SignerArgs,
    #[command(flatten)]
    contract_wasm: FixedContractWasm,
    #[command(flatten)]
    config_source: ConfigSource,
}

impl DeployRedStoneAdapter {
    #[tracing::instrument(skip_all, name = "redstone_adapter_deploy", fields(account_id = %self.signer.account_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let config = self.config_source.resolve()?;
        let loaded_contract = self
            .contract_wasm
            .load_contract::<()>(ctx, REDSTONE_ADAPTER_PACKAGE)?;
        tracing::info!(version = %loaded_contract.version, "Deploying RedStone adapter");

        let init_args = serde_json::to_vec(&serde_json::json!({ "config": config }))
            .context("serialise init args")?;
        let signer = self.signer.signer();

        ctx.batch(&signer, &self.signer.account_id)
            .deploy(&loaded_contract.wasm_bytes)
            .call(
                Function::new("new")
                    .args(init_args)
                    .max_gas(),
            )
            .transact()
            .await?;

        tracing::info!("RedStone adapter deployed");
        Ok(())
    }
}
