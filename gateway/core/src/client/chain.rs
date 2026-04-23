use near_api::{types::transaction::result::ExecutionFinalResult, Transaction};

use crate::{client::NearClient, GatewayError, GatewayResult};

#[derive(Clone, Copy)]
pub struct ChainClient<'a> {
    pub(crate) inner: &'a NearClient,
}

impl ChainClient<'_> {
    pub async fn get_transaction(
        &self,
        tx_hash: near_api::CryptoHash,
        sender_account_id: near_account_id::AccountId,
        wait_until: near_api::types::TxExecutionStatus,
    ) -> GatewayResult<ExecutionFinalResult> {
        let result = Transaction::status_with_options(sender_account_id, tx_hash, wait_until)
            .fetch_from(self.inner.network())
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?;

        Ok(result)
    }
}
