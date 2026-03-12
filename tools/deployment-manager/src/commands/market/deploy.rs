use anyhow::Context;
use near_fetch::ops::Function;
use near_sdk::NearToken;

use crate::commands::{FixedContractWasm, SignerArgs};

const MARKET_PACKAGE: &str = "templar-market-contract";
const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);

#[derive(clap::Args, Debug)]
pub struct DeployMarket {
    #[command(flatten)]
    signer: SignerArgs,
    #[command(flatten)]
    contract_wasm: FixedContractWasm,
    /// JSON-encoded init args to pass to the market contract
    #[arg(long)]
    init_args: serde_json::Value,
}

impl DeployMarket {
    #[tracing::instrument(skip_all, name = "market_deploy", fields(account_id = %self.signer.account_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let loaded_contract = self
            .contract_wasm
            .load_contract::<()>(ctx, MARKET_PACKAGE)?;
        tracing::info!(version = %loaded_contract.version, "Deploying market");

        let init_args = serde_json::to_vec(&self.init_args).context("serialise init args")?;
        let signer = self.signer.signer();

        ctx.batch(&signer, &self.signer.account_id)
            .deploy(&loaded_contract.wasm_bytes)
            .call(
                Function::new("new")
                    .args(init_args)
                    .deposit(ONE_YOCTO)
                    .max_gas(),
            )
            .transact()
            .await?;

        tracing::info!("Market deployed");
        Ok(())
    }
}
