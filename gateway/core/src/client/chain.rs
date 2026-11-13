use std::future::Future;

use near_api::types::{transaction::result::ExecutionFinalResult, Reference};
use near_api::Chain;
use near_openapi_client::types::{
    Finality, JsonRpcRequestForExperimentalProtocolConfig,
    JsonRpcRequestForExperimentalProtocolConfigMethod, JsonRpcRequestForStatus,
    JsonRpcRequestForStatusMethod,
    JsonRpcResponseForRpcProtocolConfigResponseAndRpcProtocolConfigError,
    JsonRpcResponseForRpcStatusResponseAndRpcStatusError, RpcProtocolConfigRequest,
    RpcProtocolConfigResponse, RpcStatusRequest,
};
use templar_gateway_types::{BlockSummary, ChainIdentity, CryptoHash, NearToken, ProtocolLimits};

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

    /// Exact chain identity reported by the endpoint, independent of block retention.
    pub async fn identity(&self) -> GatewayResult<ChainIdentity> {
        self.rpc(&JsonRpcRequestForStatus {
            id: "0".to_owned(),
            jsonrpc: "2.0".to_owned(),
            method: JsonRpcRequestForStatusMethod::Status,
            params: RpcStatusRequest(()),
        })
        .await
    }

    pub async fn protocol_limits(&self) -> GatewayResult<ProtocolLimits> {
        self.rpc(&protocol_config_request()).await
    }

    /// Read limits and their active protocol version from one finalized configuration.
    /// Missing version metadata is an error, not an implicit legacy-size rule.
    pub async fn protocol_limits_with_version(&self) -> GatewayResult<(ProtocolLimits, u32)> {
        self.rpc(&ProtocolLimitsWithVersionRequest(protocol_config_request()))
            .await
    }

    /// Transaction validity window in blocks from the endpoint's live protocol configuration.
    pub async fn transaction_validity_period(&self) -> GatewayResult<u64> {
        self.rpc(&TransactionValidityPeriodRequest(protocol_config_request()))
            .await
    }

    pub(crate) async fn storage_byte_cost(&self) -> GatewayResult<NearToken> {
        self.rpc(&StorageByteCostRequest(protocol_config_request()))
            .await
    }

    // near-api 0.8.6 keeps its custom-RPC retry plumbing private.
    async fn rpc<R: ChainRpcRequest + Sync>(&self, request: &R) -> GatewayResult<R::Output> {
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
                match request.query(&client).await {
                    Ok(Ok(result)) => return Ok(result),
                    Ok(Err(error)) => {
                        last_error = Some(error);
                        continue 'endpoint;
                    }
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

/// Keep RPC/validation failures distinct from transport failures: only the latter retry.
trait ChainRpcRequest {
    type Output;

    fn query(
        &self,
        client: &near_openapi_client::Client,
    ) -> impl Future<Output = Result<Result<Self::Output, String>, near_openapi_client::Error<()>>> + Send;
}

fn protocol_config_request() -> JsonRpcRequestForExperimentalProtocolConfig {
    JsonRpcRequestForExperimentalProtocolConfig {
        id: "0".to_owned(),
        jsonrpc: "2.0".to_owned(),
        method: JsonRpcRequestForExperimentalProtocolConfigMethod::ExperimentalProtocolConfig,
        params: RpcProtocolConfigRequest::Finality(Finality::Final),
    }
}

async fn query_protocol_config(
    client: &near_openapi_client::Client,
    request: &JsonRpcRequestForExperimentalProtocolConfig,
) -> Result<Result<RpcProtocolConfigResponse, String>, near_openapi_client::Error<()>> {
    Ok(
        match client
            .experimental_protocol_config(request)
            .await?
            .into_inner()
        {
            JsonRpcResponseForRpcProtocolConfigResponseAndRpcProtocolConfigError::Variant0 {
                result,
                ..
            } => Ok(result),
            JsonRpcResponseForRpcProtocolConfigResponseAndRpcProtocolConfigError::Variant1 {
                error,
                ..
            } => Err(error.to_string()),
        },
    )
}

struct StorageByteCostRequest(JsonRpcRequestForExperimentalProtocolConfig);

struct TransactionValidityPeriodRequest(JsonRpcRequestForExperimentalProtocolConfig);

struct ProtocolLimitsWithVersionRequest(JsonRpcRequestForExperimentalProtocolConfig);

impl ChainRpcRequest for ProtocolLimitsWithVersionRequest {
    type Output = (ProtocolLimits, u32);

    async fn query(
        &self,
        client: &near_openapi_client::Client,
    ) -> Result<Result<Self::Output, String>, near_openapi_client::Error<()>> {
        Ok(query_protocol_config(client, &self.0)
            .await?
            .and_then(|result| {
                let version = result.protocol_version.ok_or_else(|| {
                    "protocol config does not include protocol version".to_owned()
                })?;
                let limits =
                    protocol_limits_from_response(result).map_err(|error| error.to_string())?;
                Ok((limits, version))
            }))
    }
}

impl ChainRpcRequest for TransactionValidityPeriodRequest {
    type Output = u64;

    async fn query(
        &self,
        client: &near_openapi_client::Client,
    ) -> Result<Result<Self::Output, String>, near_openapi_client::Error<()>> {
        Ok(query_protocol_config(client, &self.0)
            .await?
            .and_then(|result| {
                result.transaction_validity_period.ok_or_else(|| {
                    "protocol config does not include transaction validity period".to_owned()
                })
            }))
    }
}

impl ChainRpcRequest for StorageByteCostRequest {
    type Output = NearToken;

    async fn query(
        &self,
        client: &near_openapi_client::Client,
    ) -> Result<Result<Self::Output, String>, near_openapi_client::Error<()>> {
        Ok(query_protocol_config(client, &self.0)
            .await?
            .and_then(|result| {
                result
                    .runtime_config
                    .and_then(|runtime| runtime.storage_amount_per_byte)
                    .ok_or_else(|| "protocol config does not include storage byte cost".to_owned())
            }))
    }
}

impl ChainRpcRequest for JsonRpcRequestForStatus {
    type Output = ChainIdentity;

    async fn query(
        &self,
        client: &near_openapi_client::Client,
    ) -> Result<Result<Self::Output, String>, near_openapi_client::Error<()>> {
        Ok(match client.status(self).await?.into_inner() {
            JsonRpcResponseForRpcStatusResponseAndRpcStatusError::Variant0 { result, .. } => {
                Ok(ChainIdentity {
                    chain_id: result.chain_id,
                    genesis_hash: CryptoHash(near_api::CryptoHash(result.genesis_hash.0)),
                })
            }
            JsonRpcResponseForRpcStatusResponseAndRpcStatusError::Variant1 { error, .. } => {
                Err(error.to_string())
            }
        })
    }
}

impl ChainRpcRequest for JsonRpcRequestForExperimentalProtocolConfig {
    type Output = ProtocolLimits;

    async fn query(
        &self,
        client: &near_openapi_client::Client,
    ) -> Result<Result<Self::Output, String>, near_openapi_client::Error<()>> {
        Ok(query_protocol_config(client, self)
            .await?
            .and_then(|result| {
                protocol_limits_from_response(result).map_err(|error| error.to_string())
            }))
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

    #[tokio::test]
    async fn rejects_missing_transaction_validity_period() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"jsonrpc":"2.0","id":"0","result":{}}"#),
            )
            .expect(1)
            .mount(&server)
            .await;
        let network = NetworkConfig::from_rpc_url("test", server.uri().parse().unwrap());
        let client = crate::NearClient::new(network);

        assert!(matches!(
            client.chain().transaction_validity_period().await,
            Err(crate::GatewayError::NearQuery(_))
        ));
    }

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
                r#"{"jsonrpc":"2.0","id":"0","error":{"name":"HANDLER_ERROR","cause":{"name":"INTERNAL_ERROR","info":{"error_message":"unsupported"}}}}"#,
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
            RPCEndpoint::new(rejected.uri().parse().unwrap()).with_retries(3);
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

    #[tokio::test]
    async fn transport_failures_exhaust_retries_before_failover() {
        let rejected = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&rejected)
            .await;
        let successful = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(LIMITS_RESPONSE))
            .mount(&successful)
            .await;

        let mut network = NetworkConfig::from_rpc_url("test", rejected.uri().parse().unwrap());
        network.rpc_endpoints[0] =
            RPCEndpoint::new(rejected.uri().parse().unwrap()).with_retries(2);
        network
            .rpc_endpoints
            .push(RPCEndpoint::new(successful.uri().parse().unwrap()).with_retries(0));
        let client = crate::NearClient::new(network);

        client.chain().protocol_limits().await.unwrap();
        assert_eq!(rejected.received_requests().await.unwrap().len(), 2);
        assert_eq!(successful.received_requests().await.unwrap().len(), 1);
    }
}
