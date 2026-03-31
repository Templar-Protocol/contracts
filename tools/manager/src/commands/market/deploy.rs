use anyhow::Context;
use near_fetch::ops::Function;

use crate::commands::{FixedContractWasm, SignerArgs};

const MARKET_PACKAGE: &str = "templar-market-contract";

#[derive(clap::Args, Debug)]
pub struct DeployMarket {
    #[command(flatten)]
    pub signer: SignerArgs,
    #[command(flatten)]
    pub contract_wasm: FixedContractWasm,
    /// JSON-encoded init args to pass to the market contract
    #[arg(long)]
    pub init_args: String,
}

impl DeployMarket {
    #[tracing::instrument(skip_all, name = "market_deploy", fields(account_id = %self.signer.account_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let loaded_contract = self
            .contract_wasm
            .load_contract::<()>(ctx, MARKET_PACKAGE)?;
        tracing::info!(version = %loaded_contract.version, "Deploying market");

        let init_args = serde_json::from_str::<serde_json::Value>(&self.init_args)
            .context("parse init args as json")?;
        let signer = self.signer.signer();

        ctx.batch(&signer, &self.signer.account_id)
            .deploy(&loaded_contract.wasm_bytes)
            .call(Function::new("new").args_json(init_args).max_gas())
            .transact()
            .await?;

        tracing::info!("Market deployed");
        Ok(())
    }
}
