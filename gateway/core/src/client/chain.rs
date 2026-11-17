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

use crate::{client::NearClient, near_rpc::RawRpcRequest, GatewayError, GatewayResult, ReadNear};

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

        let response = self
            .inner
            .rpc()
            .fetch(|| Chain::block().at(reference.clone()))
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

    async fn rpc<R: RawRpcRequest + Sync>(&self, request: &R) -> GatewayResult<R::Output> {
        self.inner
            .rpc()
            .raw(request)
            .await
            .map_err(|error| GatewayError::NearQuery(error.into_message()))
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

impl RawRpcRequest for ProtocolLimitsWithVersionRequest {
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

impl RawRpcRequest for TransactionValidityPeriodRequest {
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

impl RawRpcRequest for StorageByteCostRequest {
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

impl RawRpcRequest for JsonRpcRequestForStatus {
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

impl RawRpcRequest for JsonRpcRequestForExperimentalProtocolConfig {
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
    async fn a_refused_primary_key_reads_protocol_limits_through_the_backup() {
        use crate::near_rpc::keyed_endpoint::{self, sent_with, with_backup, BACKUP, PRIMARY};

        let server = keyed_endpoint::serving(
            ResponseTemplate::new(402),
            ResponseTemplate::new(200).set_body_string(LIMITS_RESPONSE),
        )
        .await;
        let client = crate::NearClient::new(with_backup(&server));

        assert_eq!(
            client
                .chain()
                .protocol_limits()
                .await
                .unwrap()
                .max_transaction_size,
            123
        );
        assert_eq!(sent_with(&server, PRIMARY).await, 1);
        assert_eq!(sent_with(&server, BACKUP).await, 1);
    }

    #[tokio::test]
    async fn a_refusal_outlives_a_later_endpoints_failure() {
        use crate::{
            near_rpc::keyed_endpoint::{self, BACKUP},
            NearRpc,
        };

        let keyed = keyed_endpoint::serving(
            ResponseTemplate::new(402),
            ResponseTemplate::new(200).set_body_string(LIMITS_RESPONSE),
        )
        .await;
        let failing = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&failing)
            .await;
        let mut network = keyed_endpoint::network(&keyed, 1);
        network
            .rpc_endpoints
            .push(RPCEndpoint::new(failing.uri().parse().unwrap()).with_retries(1));
        let client = crate::NearClient::new(NearRpc::with_backup_api_key(network, BACKUP));

        client.chain().protocol_limits().await.unwrap();
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
