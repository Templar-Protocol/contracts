use near_fetch::ops::Function;

use crate::commands::{json_input::InitArgsSource, FixedContractWasm, SignerArgs};

const MARKET_PACKAGE: &str = "templar-market-contract";

#[derive(clap::Args, Debug)]
pub struct DeployMarket {
    #[command(flatten)]
    pub signer: SignerArgs,
    #[command(flatten)]
    pub contract_wasm: FixedContractWasm,
    #[command(flatten)]
    pub init_args_source: InitArgsSource,
}

impl DeployMarket {
    #[tracing::instrument(skip_all, name = "market_deploy", fields(account_id = %self.signer.account_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let loaded_contract = self
            .contract_wasm
            .load_contract::<()>(ctx, MARKET_PACKAGE)?;
        tracing::info!(version = %loaded_contract.version, "Deploying market");

        let init_args = self.init_args_source.parse()?;
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
