use near_api::types::transaction::result::ExecutionFinalResult;
use near_jsonrpc_client::{methods, JsonRpcClient};
use templar_gateway_types::{CryptoHash, NearToken};

use crate::{client::NearClient, GatewayError, GatewayResult, ReadNear};

#[derive(Clone, Copy)]
pub struct ChainClient<'a> {
    pub(crate) inner: &'a NearClient,
}

impl ChainClient<'_> {
    /// Connect a JSON-RPC client to the network's primary endpoint.
    ///
    /// `near_api` exposes no builder for chain-level reads like gas price, so
    /// these go through `near-jsonrpc-client` directly against the configured
    /// endpoint (whose URL already carries any auth, e.g. an `?apiKey=` param).
    fn json_rpc(self) -> GatewayResult<JsonRpcClient> {
        let endpoint = self
            .inner
            .network()
            .rpc_endpoints
            .first()
            .ok_or_else(|| GatewayError::NearQuery("no RPC endpoint configured".to_owned()))?;
        Ok(JsonRpcClient::connect(endpoint.url.as_str()))
    }

    /// The current gas price (yoctoNEAR per unit of gas) at the latest block.
    pub async fn gas_price(&self) -> GatewayResult<NearToken> {
        let response = self
            .json_rpc()?
            .call(methods::gas_price::RpcGasPriceRequest { block_id: None })
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?;
        Ok(NearToken::from_yoctonear(response.gas_price.as_yoctonear()))
    }

    /// Header height and timestamp (nanoseconds) for a block; `block_hash`
    /// selects a specific block, otherwise the latest final block is used.
    pub async fn block(&self, block_hash: Option<CryptoHash>) -> GatewayResult<(u64, u64)> {
        use near_primitives::types::{BlockId, BlockReference, Finality};

        let block_reference = match block_hash {
            // Both are 32-byte hashes; convert byte-for-byte.
            Some(hash) => {
                BlockReference::BlockId(BlockId::Hash(near_primitives::hash::CryptoHash(hash.0 .0)))
            }
            None => BlockReference::Finality(Finality::Final),
        };
        let response = self
            .json_rpc()?
            .call(methods::block::RpcBlockRequest { block_reference })
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?;
        Ok((response.header.height, response.header.timestamp_nanosec))
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
