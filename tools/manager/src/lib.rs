pub mod batch;
pub mod commands;
pub mod gateway_cli;
pub mod util;

pub use templar_tools_common::near;

#[allow(async_fn_in_trait)]
pub trait Runner<Input> {
    type Output;

    async fn run(&self, ctx: &crate::CliContext, input: &Input) -> anyhow::Result<Self::Output>;
}

pub struct CliContext {
    pub transaction_url_prefix: String,
    pub near: near_jsonrpc_client::JsonRpcClient,
}

impl CliContext {
    /// Create a [`batch::BoundBatch`] that automatically logs the transaction hash and
    /// propagates execution failures when [`batch::BoundBatch::transact`] is called.
    pub fn batch<'a>(
        &'a self,
        signer: &'a near_crypto::Signer,
        receiver_id: &near_sdk::AccountId,
    ) -> batch::BoundBatch<'a> {
        batch::BoundBatch::new(
            self.transaction_url_prefix.clone(),
            &self.near,
            signer,
            receiver_id,
        )
    }
}

pub async fn run() -> anyhow::Result<()> {
    gateway_cli::run().await
}
