use near_fetch::ops::Function;

use crate::commands::{FixedContractWasm, SignerArgs};

const PROXY_ORACLE_PACKAGE: &str = "templar-proxy-oracle-contract";

#[derive(clap::Args, Debug)]
pub struct DeployProxyOracle {
    #[command(flatten)]
    pub signer: SignerArgs,
    #[command(flatten)]
    pub contract_wasm: FixedContractWasm,
}

impl DeployProxyOracle {
    #[tracing::instrument(skip_all, name = "proxy_oracle_deploy", fields(account_id = %self.signer.account_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let loaded_contract = self
            .contract_wasm
            .load_contract::<()>(ctx, PROXY_ORACLE_PACKAGE)?;
        tracing::info!(version = %loaded_contract.version, "Deploying proxy oracle");

        let signer = self.signer.signer();

        ctx.batch(&signer, &self.signer.account_id)
            .deploy(&loaded_contract.wasm_bytes)
            .call(
                Function::new("new")
                    .args_json(serde_json::json!({}))
                    .max_gas(),
            )
            .transact()
            .await?;

        tracing::info!("Proxy oracle deployed");
        Ok(())
    }
}
