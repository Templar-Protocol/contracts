//! NEAR Intents Bridge API client
//!
//! HTTP client for interacting with NEAR Intents Bridge JSON-RPC API
//! at https://bridge.chaindefuser.com/rpc

use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::error::{BridgeError, BridgeResult};

use super::models::*;

/// Default mainnet bridge API endpoint
pub const MAINNET_BRIDGE_API: &str = "https://bridge.chaindefuser.com/rpc";

/// Token cache entry
#[derive(Debug, Clone)]
struct CachedTokens {
    tokens: Vec<TokenInfo>,
    cached_at: std::time::Instant,
}

/// Bridge API client with caching
#[derive(Clone)]
pub struct BridgeClient {
    endpoint: String,
    http_client: Client,
    /// Cache for supported tokens by chain
    token_cache: Arc<RwLock<HashMap<String, CachedTokens>>>,
    /// Cache TTL in seconds
    cache_ttl_secs: u64,
}

impl BridgeClient {
    /// Create new bridge client with default endpoint
    pub fn new_mainnet() -> Self {
        Self::new(MAINNET_BRIDGE_API.to_string())
    }

    /// Create new bridge client with custom endpoint
    pub fn new(endpoint: String) -> Self {
        Self {
            endpoint,
            http_client: Client::new(),
            token_cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl_secs: 300, // 5 minute cache
        }
    }

    /// Set cache TTL in seconds
    pub fn with_cache_ttl(mut self, ttl_secs: u64) -> Self {
        self.cache_ttl_secs = ttl_secs;
        self
    }

    /// Get supported tokens for one or more chains
    ///
    /// Chains should be in format "eth:1", "eth:42161", etc.
    pub async fn get_supported_tokens(&self, chains: &[String]) -> BridgeResult<Vec<TokenInfo>> {
        debug!(chains = ?chains, "Getting supported tokens");

        // Check cache first
        let cache_key = chains.join(",");
        {
            let cache = self.token_cache.read().await;
            if let Some(cached) = cache.get(&cache_key) {
                if cached.cached_at.elapsed().as_secs() < self.cache_ttl_secs {
                    debug!("Returning cached tokens");
                    return Ok(cached.tokens.clone());
                }
            }
        }

        let params = SupportedTokensParams {
            chains: chains.to_vec(),
        };
        let request = JsonRpcRequest::new("supported_tokens", params);

        let response: JsonRpcResponse<SupportedTokensResult> = self.send_request(request).await?;
        let result = response
            .result
            .ok_or_else(|| BridgeError::ApiError("No result in response".to_string()))?;

        // Update cache
        {
            let mut cache = self.token_cache.write().await;
            cache.insert(
                cache_key,
                CachedTokens {
                    tokens: result.tokens.clone(),
                    cached_at: std::time::Instant::now(),
                },
            );
        }

        info!(
            token_count = result.tokens.len(),
            "Fetched supported tokens from bridge"
        );
        Ok(result.tokens)
    }

    /// Get deposit address for cross-chain deposit
    ///
    /// This generates a unique deposit address for the given NEAR account.
    /// The user must send tokens to this address on the external chain.
    pub async fn get_deposit_address(
        &self,
        account_id: &str,
        chain: &str,
    ) -> BridgeResult<DepositAddressResult> {
        debug!(
            account_id = %account_id,
            chain = %chain,
            "Getting deposit address"
        );

        let params = DepositAddressParams {
            account_id: account_id.to_string(),
            chain: chain.to_string(),
        };
        let request = JsonRpcRequest::new("deposit_address", params);

        let response: JsonRpcResponse<DepositAddressResult> = self.send_request(request).await?;
        let result = response.result.ok_or_else(|| {
            BridgeError::DepositAddressFailed("No result in response".to_string())
        })?;

        info!(
            address = %result.address,
            chain = %result.chain,
            "Generated deposit address"
        );
        Ok(result)
    }

    /// Notify bridge of a deposit transaction
    ///
    /// This speeds up deposit processing by informing the bridge
    /// about a transaction before it's automatically detected.
    pub async fn notify_deposit(&self, tx_hash: &str, chain: &str) -> BridgeResult<bool> {
        debug!(
            tx_hash = %tx_hash,
            chain = %chain,
            "Notifying bridge of deposit"
        );

        let params = NotifyDepositParams {
            tx_hash: tx_hash.to_string(),
            chain: chain.to_string(),
        };
        let request = JsonRpcRequest::new("notify_deposit", params);

        let response: JsonRpcResponse<NotifyDepositResult> = self.send_request(request).await?;
        let result = response
            .result
            .ok_or_else(|| BridgeError::ApiError("No result in response".to_string()))?;

        info!(
            tx_hash = %tx_hash,
            acknowledged = %result.acknowledged,
            "Bridge acknowledged deposit notification"
        );
        Ok(result.acknowledged)
    }

    /// Get recent deposits for an account
    pub async fn get_recent_deposits(
        &self,
        account_id: &str,
        limit: Option<u32>,
    ) -> BridgeResult<Vec<DepositInfo>> {
        debug!(
            account_id = %account_id,
            limit = ?limit,
            "Getting recent deposits"
        );

        let params = RecentDepositsParams {
            account_id: account_id.to_string(),
            limit,
        };
        let request = JsonRpcRequest::new("recent_deposits", params);

        let response: JsonRpcResponse<RecentDepositsResult> = self.send_request(request).await?;
        let result = response
            .result
            .ok_or_else(|| BridgeError::ApiError("No result in response".to_string()))?;

        info!(
            deposit_count = result.deposits.len(),
            "Fetched recent deposits"
        );
        Ok(result.deposits)
    }

    /// Find token info by asset name and chain
    pub async fn find_token(
        &self,
        asset_name: &str,
        chain: &str,
    ) -> BridgeResult<Option<TokenInfo>> {
        let tokens = self.get_supported_tokens(&[chain.to_string()]).await?;

        let token = tokens.into_iter().find(|t| {
            t.asset_name.to_lowercase() == asset_name.to_lowercase()
                && t.chain().as_deref() == Some(chain)
        });

        Ok(token)
    }

    /// Check if the bridge API is reachable
    pub async fn health_check(&self) -> bool {
        // Try to fetch supported tokens for a chain
        match self.get_supported_tokens(&["eth:1".to_string()]).await {
            Ok(tokens) => !tokens.is_empty(),
            Err(e) => {
                warn!(error = %e, "Bridge health check failed");
                false
            }
        }
    }

    /// Send JSON-RPC request to bridge API
    async fn send_request<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        request: JsonRpcRequest<T>,
    ) -> BridgeResult<JsonRpcResponse<R>> {
        let response = self
            .http_client
            .post(&self.endpoint)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            warn!(
                status = %response.status(),
                "Bridge API returned non-success status"
            );
            return Err(BridgeError::ApiError(format!("HTTP {}", response.status())));
        }

        let json_response: JsonRpcResponse<R> = response.json().await?;

        if let Some(error) = json_response.error {
            return Err(BridgeError::ApiError(format!(
                "JSON-RPC error {}: {}",
                error.code(),
                error.message()
            )));
        }

        Ok(json_response)
    }

    /// Get the endpoint URL
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Clear the token cache
    pub async fn clear_cache(&self) {
        let mut cache = self.token_cache.write().await;
        cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    async fn setup_mock_server() -> (MockServer, BridgeClient) {
        let mock_server = MockServer::start().await;
        let client = BridgeClient::new(mock_server.uri());
        (mock_server, client)
    }

    #[tokio::test]
    async fn test_get_supported_tokens_real_format() {
        let (mock_server, client) = setup_mock_server().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1,
                "jsonrpc": "2.0",
                "result": {
                    "tokens": [
                        {
                            "defuse_asset_identifier": "eth:1:native",
                            "near_token_id": "eth.omft.near",
                            "decimals": 18,
                            "asset_name": "ETH",
                            "min_deposit_amount": "1",
                            "withdrawal_fee": "35000000000000"
                        },
                        {
                            "defuse_asset_identifier": "eth:1:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
                            "near_token_id": "eth-0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48.omft.near",
                            "decimals": 6,
                            "asset_name": "USDC",
                            "withdrawal_fee": "300000"
                        }
                    ]
                }
            })))
            .mount(&mock_server)
            .await;

        let tokens = client
            .get_supported_tokens(&["eth:1".to_string()])
            .await
            .unwrap();

        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].asset_name, "ETH");
        assert_eq!(tokens[0].near_token_id, "eth.omft.near");
        assert!(tokens[0].is_native());
        assert_eq!(tokens[1].asset_name, "USDC");
        assert_eq!(tokens[1].decimals, 6);
    }

    #[tokio::test]
    async fn test_get_deposit_address_real_format() {
        let (mock_server, client) = setup_mock_server().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1,
                "jsonrpc": "2.0",
                "result": {
                    "address": "0xbA5C6ABBAe64AD465d104949CC150011C1777eFB",
                    "chain": "eth:1"
                }
            })))
            .mount(&mock_server)
            .await;

        let result = client
            .get_deposit_address("tmplr-liq.near", "eth:1")
            .await
            .unwrap();

        assert_eq!(result.address, "0xbA5C6ABBAe64AD465d104949CC150011C1777eFB");
        assert_eq!(result.chain, "eth:1");
    }

    #[tokio::test]
    async fn test_notify_deposit() {
        let (mock_server, client) = setup_mock_server().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1,
                "jsonrpc": "2.0",
                "result": {
                    "acknowledged": true
                }
            })))
            .mount(&mock_server)
            .await;

        let acknowledged = client.notify_deposit("0xabc123", "eth:1").await.unwrap();

        assert!(acknowledged);
    }

    #[tokio::test]
    async fn test_get_recent_deposits() {
        let (mock_server, client) = setup_mock_server().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1,
                "jsonrpc": "2.0",
                "result": {
                    "deposits": [
                        {
                            "tx_hash": "0xabc123",
                            "amount": "1000000",
                            "status": "COMPLETED",
                            "defuse_asset_identifier": "eth:1:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
                            "chain": "eth:1",
                            "near_tx_hash": "ABC123",
                            "completed_at": "2025-11-14T12:00:00Z"
                        }
                    ]
                }
            })))
            .mount(&mock_server)
            .await;

        let deposits = client
            .get_recent_deposits("test.near", Some(10))
            .await
            .unwrap();

        assert_eq!(deposits.len(), 1);
        assert_eq!(deposits[0].tx_hash, "0xabc123");
        assert_eq!(deposits[0].status, DepositStatus::Completed);
        assert_eq!(deposits[0].near_tx_hash, Some("ABC123".to_string()));
    }

    #[tokio::test]
    async fn test_find_token() {
        let (mock_server, client) = setup_mock_server().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1,
                "jsonrpc": "2.0",
                "result": {
                    "tokens": [
                        {
                            "defuse_asset_identifier": "eth:1:native",
                            "near_token_id": "eth.omft.near",
                            "decimals": 18,
                            "asset_name": "ETH"
                        },
                        {
                            "defuse_asset_identifier": "eth:1:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
                            "near_token_id": "usdc.omft.near",
                            "decimals": 6,
                            "asset_name": "USDC"
                        }
                    ]
                }
            })))
            .mount(&mock_server)
            .await;

        let usdc = client.find_token("USDC", "eth:1").await.unwrap();
        assert!(usdc.is_some());
        let usdc = usdc.unwrap();
        assert_eq!(usdc.asset_name, "USDC");
        assert_eq!(usdc.decimals, 6);

        // Case insensitive
        let eth = client.find_token("eth", "eth:1").await.unwrap();
        assert!(eth.is_some());
    }

    #[tokio::test]
    async fn test_token_caching() {
        let (mock_server, client) = setup_mock_server().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1,
                "jsonrpc": "2.0",
                "result": {
                    "tokens": [
                        {
                            "defuse_asset_identifier": "eth:1:native",
                            "near_token_id": "eth.omft.near",
                            "decimals": 18,
                            "asset_name": "ETH"
                        }
                    ]
                }
            })))
            .expect(1) // Should only be called once due to caching
            .mount(&mock_server)
            .await;

        // First call - hits API
        let tokens1 = client
            .get_supported_tokens(&["eth:1".to_string()])
            .await
            .unwrap();
        assert_eq!(tokens1.len(), 1);

        // Second call - uses cache
        let tokens2 = client
            .get_supported_tokens(&["eth:1".to_string()])
            .await
            .unwrap();
        assert_eq!(tokens2.len(), 1);
    }

    #[tokio::test]
    async fn test_bridge_api_error_response() {
        let (mock_server, client) = setup_mock_server().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1,
                "jsonrpc": "2.0",
                "error": {
                    "code": -32600,
                    "message": "Invalid request"
                }
            })))
            .mount(&mock_server)
            .await;

        let result = client.get_supported_tokens(&["eth:1".to_string()]).await;

        assert!(result.is_err());
        match result {
            Err(BridgeError::ApiError(msg)) => {
                assert!(msg.contains("Invalid request") || msg.contains("-32600"));
            }
            _ => panic!("Expected ApiError"),
        }
    }

    #[tokio::test]
    async fn test_client_creation_mainnet() {
        let client = BridgeClient::new_mainnet();
        assert_eq!(client.endpoint(), MAINNET_BRIDGE_API);
    }

    #[tokio::test]
    async fn test_client_creation_custom() {
        let client = BridgeClient::new("https://custom.bridge.api/rpc".to_string());
        assert_eq!(client.endpoint(), "https://custom.bridge.api/rpc");
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let (mock_server, client) = setup_mock_server().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1,
                "jsonrpc": "2.0",
                "result": {
                    "tokens": []
                }
            })))
            .expect(2) // Called twice because cache is cleared
            .mount(&mock_server)
            .await;

        // First call
        let _ = client
            .get_supported_tokens(&["eth:1".to_string()])
            .await
            .unwrap();

        // Clear cache
        client.clear_cache().await;

        // Second call hits API again
        let _ = client
            .get_supported_tokens(&["eth:1".to_string()])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_empty_deposits() {
        let (mock_server, client) = setup_mock_server().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1,
                "jsonrpc": "2.0",
                "result": {
                    "deposits": []
                }
            })))
            .mount(&mock_server)
            .await;

        let deposits = client.get_recent_deposits("test.near", None).await.unwrap();

        assert!(deposits.is_empty());
    }
}
