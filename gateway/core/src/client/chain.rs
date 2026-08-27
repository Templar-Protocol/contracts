use near_api::types::{transaction::result::ExecutionFinalResult, Reference};
use near_api::Chain;
use near_openapi_client::types::{
    Finality, JsonRpcRequestForExperimentalProtocolConfig,
    JsonRpcRequestForExperimentalProtocolConfigMethod,
    JsonRpcResponseForRpcProtocolConfigResponseAndRpcProtocolConfigError, RpcProtocolConfigRequest,
    RpcProtocolConfigResponse,
};
use templar_gateway_types::{BlockSummary, CryptoHash, ProtocolLimits};

use crate::{client::NearClient, GatewayError, GatewayResult, ReadNear};

const RPC_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Clone, Copy)]
pub struct ChainClient<'a> {
    pub(crate) inner: &'a NearClient,
}

impl ChainClient<'_> {
    /// Header summary for a block; `block_hash` selects a specific block,
    /// otherwise the client's configured query finality is used.
    pub async fn block(&self, block_hash: Option<CryptoHash>) -> GatewayResult<BlockSummary> {
        let reference = match block_hash {
            Some(hash) => Reference::AtBlockHash(hash.0),
            None => self.inner.finality_policy().query_reference(),
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
    // near-api 0.8.6 keeps its custom-RPC retry plumbing private.
    pub async fn protocol_limits(&self) -> GatewayResult<ProtocolLimits> {
        let request = JsonRpcRequestForExperimentalProtocolConfig {
            id: "0".to_owned(),
            jsonrpc: "2.0".to_owned(),
            method: JsonRpcRequestForExperimentalProtocolConfigMethod::ExperimentalProtocolConfig,
            params: RpcProtocolConfigRequest::Finality(Finality::Final),
        };
        let mut last_error = None;

        for endpoint in &self.inner.network().rpc_endpoints {
            let mut headers = reqwest::header::HeaderMap::new();
            if let Some(value) = &endpoint.bearer_header {
                let mut header = reqwest::header::HeaderValue::from_str(value)
                    .map_err(|error| GatewayError::NearQuery(error.to_string()))?;
                header.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, header.clone());
                headers.insert(
                    reqwest::header::HeaderName::from_static("x-api-key"),
                    header,
                );
            }
            let http = reqwest::ClientBuilder::new()
                .connect_timeout(RPC_REQUEST_TIMEOUT)
                .timeout(RPC_REQUEST_TIMEOUT)
                .default_headers(headers)
                .build()
                .map_err(|error| GatewayError::NearQuery(error.to_string()))?;
            let client = near_openapi_client::Client::new_with_client(
                endpoint.url.as_str().trim_end_matches('/'),
                http,
            );

            for attempt in 0..endpoint.retries {
                match client.experimental_protocol_config(&request).await {
                    Ok(response) => {
                        return match response.into_inner() {
                            JsonRpcResponseForRpcProtocolConfigResponseAndRpcProtocolConfigError::Variant0 {
                                result,
                                ..
                            } => protocol_limits_from_response(result),
                            JsonRpcResponseForRpcProtocolConfigResponseAndRpcProtocolConfigError::Variant1 {
                                error,
                                ..
                            } => Err(GatewayError::NearQuery(error.to_string())),
                        };
                    }
                    Err(error) => {
                        last_error = Some(error.to_string());
                        tokio::time::sleep(endpoint.get_sleep_duration(attempt as usize)).await;
                    }
                }
            }
        }

        Err(GatewayError::NearQuery(last_error.unwrap_or_else(|| {
            "no RPC endpoints are configured".to_owned()
        })))
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

fn protocol_limits_from_response(
    response: RpcProtocolConfigResponse,
) -> GatewayResult<ProtocolLimits> {
    response
        .runtime_config
        .and_then(|runtime| runtime.wasm_config)
        .and_then(|wasm| wasm.limit_config)
        .and_then(|limit| {
            Some(ProtocolLimits {
                max_transaction_size: limit.max_transaction_size?,
                max_total_prepaid_gas: limit.max_total_prepaid_gas?,
            })
        })
        .ok_or_else(|| {
            GatewayError::NearQuery(
                "protocol config does not include required transaction limits".to_owned(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::protocol_limits_from_response;
    use templar_gateway_types::ProtocolLimits;

    #[test]
    fn extracts_protocol_transaction_limits() {
        let response: near_openapi_client::types::RpcProtocolConfigResponse =
            serde_json::from_str(
                r#"{"runtime_config":{"wasm_config":{"limit_config":{"max_transaction_size":123,"max_total_prepaid_gas":"456"}}}}"#,
            )
            .unwrap();
        assert_eq!(
            protocol_limits_from_response(response).unwrap(),
            ProtocolLimits {
                max_transaction_size: 123,
                max_total_prepaid_gas: templar_gateway_types::NearGas::from_gas(456),
            }
        );
    }

    #[test]
    fn rejects_missing_protocol_transaction_limits() {
        let response: near_openapi_client::types::RpcProtocolConfigResponse =
            serde_json::from_str(r#"{"runtime_config":{}}"#).unwrap();
        let error = protocol_limits_from_response(response).expect_err("limits are required");
        assert!(matches!(
            &error,
            crate::GatewayError::NearQuery(message)
                if message == "protocol config does not include required transaction limits"
        ));
    }
}
