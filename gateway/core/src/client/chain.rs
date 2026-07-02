use near_api::types::transaction::result::ExecutionFinalResult;
use near_api::types::Reference;
use near_api::Chain;
use templar_gateway_types::{BlockSummary, CryptoHash};

use crate::{client::NearClient, GatewayError, GatewayResult, ReadNear};

#[derive(Clone, Copy)]
pub struct ChainClient<'a> {
    pub(crate) inner: &'a NearClient,
}

impl ChainClient<'_> {
    /// Header summary for a block; `block_hash` selects a specific block,
    /// otherwise the latest final block is used.
    pub async fn block(&self, block_hash: Option<CryptoHash>) -> GatewayResult<BlockSummary> {
        let reference = match block_hash {
            Some(hash) => Reference::AtBlockHash(hash.0),
            None => Reference::Final,
        };

        let response = Chain::block()
            .at(reference)
            .fetch_from(self.inner.network())
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?;

        let header = response.header;
        // `timestamp_nanosec` is wire-encoded as a decimal string.
        let timestamp_ns = header.timestamp_nanosec.parse::<u64>().map_err(|error| {
            GatewayError::NearQuery(format!("invalid block timestamp: {error}"))
        })?;

        Ok(BlockSummary {
            height: header.height,
            timestamp_ns,
            // `near_openapi_types::NearToken` is `near_token::NearToken`.
            gas_price: header.gas_price,
            // Both `CryptoHash`es are `[u8; 32]`; convert byte-for-byte.
            hash: CryptoHash(near_api::CryptoHash(header.hash.0)),
        })
    }

    pub async fn get_transaction(
        &self,
        tx_hash: near_api::CryptoHash,
        sender_account_id: near_account_id::AccountId,
        wait_until: near_api::types::TxExecutionStatus,
    ) -> GatewayResult<ExecutionFinalResult> {
        <NearClient as ReadNear>::view_transaction_status(
            self.inner,
            sender_account_id,
            tx_hash,
            wait_until,
        )
        .await
    }
}
