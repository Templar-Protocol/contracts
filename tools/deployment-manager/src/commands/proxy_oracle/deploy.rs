use near_fetch::ops::Function;
use near_sdk::NearToken;

use crate::commands::{FixedContractWasm, SignerArgs};

const PROXY_ORACLE_PACKAGE: &str = "templar-proxy-oracle-contract";
const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);

#[derive(clap::Args, Debug)]
pub struct DeployProxyOracle {
    #[command(flatten)]
    signer: SignerArgs,
    #[command(flatten)]
    contract_wasm: FixedContractWasm,
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
                    .deposit(ONE_YOCTO)
                    .max_gas(),
            )
            .transact()
            .await?;

        tracing::info!("Proxy oracle deployed");
        Ok(())
    }
}
