use near_fetch::ops::Function;

use crate::commands::{ContractWasm, SignerArgs};

#[derive(clap::Args, Debug)]
pub struct Native {
    #[command(flatten)]
    pub contract_wasm: ContractWasm,
}

impl Native {
    pub fn new(contract_wasm: ContractWasm) -> Self {
        Self { contract_wasm }
    }

    #[tracing::instrument(name = "deploy_standalone", skip_all, fields(%default_package))]
    pub async fn run(
        &self,
        ctx: &crate::CliContext,
        signer_args: &SignerArgs,
        init_args: Vec<u8>,
        default_package: &str,
    ) -> anyhow::Result<()> {
        let loaded_contract = self
            .contract_wasm
            .load_contract::<()>(ctx, default_package)?;
        tracing::info!(version = %loaded_contract.version, "Deploying standalone contract");

        let signer = signer_args.signer();

        ctx.batch(&signer, &signer_args.account_id)
            .deploy(&loaded_contract.wasm_bytes)
            .call(Function::new("new").args(init_args).max_gas())
            .transact()
            .await?;

        tracing::info!("Contract deployed");
        Ok(())
    }
}
