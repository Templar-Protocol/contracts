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

        'endpoint: for endpoint in &self.inner.network().rpc_endpoints {
            let mut headers = reqwest::header::HeaderMap::new();
            if let Some(value) = &endpoint.bearer_header {
                let header = match reqwest::header::HeaderValue::from_str(value) {
                    Ok(header) => header,
                    Err(error) => {
                        last_error = Some(error.to_string());
                        continue 'endpoint;
                    }
                };
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

            let attempts = usize::from(endpoint.retries).max(1);
            for attempt in 0..attempts {
                match client.experimental_protocol_config(&request).await {
                    Ok(response) => match response.into_inner() {
                        JsonRpcResponseForRpcProtocolConfigResponseAndRpcProtocolConfigError::Variant0 {
                            result,
                            ..
                        } => match protocol_limits_from_response(result) {
                            Ok(limits) => return Ok(limits),
                            Err(error) => {
                                last_error = Some(error.to_string());
                                continue 'endpoint;
                            }
                        },
                        JsonRpcResponseForRpcProtocolConfigResponseAndRpcProtocolConfigError::Variant1 {
                            error,
                            ..
                        } => {
                            last_error = Some(error.to_string());
                            continue 'endpoint;
                        }
                    },
                    Err(error) => {
                        last_error = Some(error.to_string());
                        if attempt + 1 < attempts {
                            tokio::time::sleep(endpoint.get_sleep_duration(attempt)).await;
                        }
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
        .and_then(|runtime| {
            let limit = runtime.wasm_config?.limit_config?;
            let storage = runtime.transaction_costs?.storage_usage_config?;
            Some(ProtocolLimits {
                max_transaction_size: limit.max_transaction_size?,
                max_total_prepaid_gas: limit.max_total_prepaid_gas?,
                max_length_storage_key: limit.max_length_storage_key?,
                max_length_storage_value: limit.max_length_storage_value?,
                num_bytes_account: storage.num_bytes_account?,
                num_extra_bytes_record: storage.num_extra_bytes_record?,
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
    use near_api::{NetworkConfig, RPCEndpoint};
    use templar_gateway_types::ProtocolLimits;
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

    const LIMITS_RESPONSE: &str = r#"{"jsonrpc":"2.0","id":"0","result":{"runtime_config":{"transaction_costs":{"storage_usage_config":{"num_bytes_account":100,"num_extra_bytes_record":40}},"wasm_config":{"limit_config":{"max_transaction_size":123,"max_total_prepaid_gas":"456","max_length_storage_key":789,"max_length_storage_value":1011}}}}}"#;

    #[test]
    fn extracts_protocol_transaction_limits() {
        let response: near_openapi_client::types::RpcProtocolConfigResponse =
            serde_json::from_str(
                r#"{"runtime_config":{"transaction_costs":{"storage_usage_config":{"num_bytes_account":100,"num_extra_bytes_record":40}},"wasm_config":{"limit_config":{"max_transaction_size":123,"max_total_prepaid_gas":"456","max_length_storage_key":789,"max_length_storage_value":1011}}}}"#,
            )
            .unwrap();
        assert_eq!(
            protocol_limits_from_response(response).unwrap(),
            ProtocolLimits {
                max_transaction_size: 123,
                max_total_prepaid_gas: templar_gateway_types::NearGas::from_gas(456),
                max_length_storage_key: 789,
                max_length_storage_value: 1011,
                num_bytes_account: 100,
                num_extra_bytes_record: 40,
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

    #[tokio::test]
    async fn protocol_rpc_errors_fail_over_to_the_next_endpoint() {
        let rejected = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"jsonrpc":"2.0","id":"0","error":{"name":"INTERNAL_ERROR","info":{"error_message":"unsupported"}}}"#,
            ))
            .mount(&rejected)
            .await;
        let successful = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(LIMITS_RESPONSE))
            .mount(&successful)
            .await;

        let mut network = NetworkConfig::from_rpc_url("test", rejected.uri().parse().unwrap());
        network.rpc_endpoints[0] =
            RPCEndpoint::new(rejected.uri().parse().unwrap()).with_retries(0);
        network
            .rpc_endpoints
            .push(RPCEndpoint::new(successful.uri().parse().unwrap()).with_retries(0));
        let client = crate::NearClient::new(network);

        assert_eq!(
            client.chain().protocol_limits().await.unwrap(),
            ProtocolLimits {
                max_transaction_size: 123,
                max_total_prepaid_gas: templar_gateway_types::NearGas::from_gas(456),
                max_length_storage_key: 789,
                max_length_storage_value: 1011,
                num_bytes_account: 100,
                num_extra_bytes_record: 40,
            }
        );
        assert_eq!(rejected.received_requests().await.unwrap().len(), 1);
        assert_eq!(successful.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn incomplete_protocol_response_fails_over_to_next_endpoint() {
        let incomplete = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"jsonrpc":"2.0","id":"0","result":{"runtime_config":{}}}"#,
                ),
            )
            .mount(&incomplete)
            .await;
        let successful = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(LIMITS_RESPONSE))
            .mount(&successful)
            .await;

        let mut network = NetworkConfig::from_rpc_url("test", incomplete.uri().parse().unwrap());
        network.rpc_endpoints[0] =
            RPCEndpoint::new(incomplete.uri().parse().unwrap()).with_retries(0);
        network
            .rpc_endpoints
            .push(RPCEndpoint::new(successful.uri().parse().unwrap()).with_retries(0));
        let client = crate::NearClient::new(network);

        assert_eq!(
            client
                .chain()
                .protocol_limits()
                .await
                .unwrap()
                .max_transaction_size,
            123
        );
        assert_eq!(incomplete.received_requests().await.unwrap().len(), 1);
        assert_eq!(successful.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn invalid_endpoint_headers_fail_over_to_the_next_endpoint() {
        let rejected = MockServer::start().await;
        let successful = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(LIMITS_RESPONSE))
            .mount(&successful)
            .await;

        let mut network = NetworkConfig::from_rpc_url("test", rejected.uri().parse().unwrap());
        network.rpc_endpoints[0] =
            RPCEndpoint::new(rejected.uri().parse().unwrap()).with_retries(0);
        network.rpc_endpoints[0].bearer_header = Some("Bearer invalid\nvalue".to_owned());
        network
            .rpc_endpoints
            .push(RPCEndpoint::new(successful.uri().parse().unwrap()).with_retries(0));
        let client = crate::NearClient::new(network);

        client.chain().protocol_limits().await.unwrap();
        assert_eq!(rejected.received_requests().await.unwrap().len(), 0);
        assert_eq!(successful.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn zero_retries_still_makes_one_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(LIMITS_RESPONSE))
            .mount(&server)
            .await;
        let mut network = NetworkConfig::from_rpc_url("test", server.uri().parse().unwrap());
        network.rpc_endpoints[0] = RPCEndpoint::new(server.uri().parse().unwrap()).with_retries(0);
        let client = crate::NearClient::new(network);

        client.chain().protocol_limits().await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}
