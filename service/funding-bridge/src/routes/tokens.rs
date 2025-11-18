//! Token endpoint - Look up token information and OMFT IDs

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::app::App;

/// Query parameters for token lookup
#[derive(Debug, Deserialize)]
pub struct TokenLookupQuery {
    /// Asset name (e.g., "USDT", "USDC", "ETH")
    pub asset: String,
    /// Destination chain (e.g., "eth:1", "ethereum", "arbitrum")
    pub chain: String,
}

/// Response for token lookup
#[derive(Debug, Serialize)]
pub struct TokenLookupResponse {
    /// Original asset name
    pub asset: String,
    /// Destination chain (normalized)
    pub chain: String,
    /// OMFT token ID for use in NEAR
    pub omft_token_id: String,
    /// Token decimals (if known)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,
    /// Bridge API token info (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_info: Option<BridgeTokenInfo>,
}

/// Bridge API token information
#[derive(Debug, Serialize)]
pub struct BridgeTokenInfo {
    /// Asset name from bridge
    pub asset_name: String,
    /// Chain type
    pub chain_type: String,
    /// Chain ID
    pub chain_id: String,
    /// Token decimals
    pub decimals: u8,
    /// Defuse asset identifier
    pub defuse_asset_identifier: String,
}

/// GET /tokens/lookup - Look up OMFT token ID for an asset
///
/// Returns the OMFT token ID and other token information for use in
/// withdrawal intents and cross-chain operations.
#[tracing::instrument(
    name = "token_lookup",
    skip(app),
    fields(
        asset = %query.asset,
        chain = %query.chain
    )
)]
pub async fn token_lookup(
    State(app): State<App>,
    Query(query): Query<TokenLookupQuery>,
) -> Response {
    info!("Looking up token information");

    // Normalize chain ID
    let chain_str = crate::routes::deposit::normalize_chain_id(&query.chain);

    // Try to get token info from bridge API first
    let (bridge_info, decimals, bridge_omft_id) =
        match app.bridge_client.find_token(&query.asset, &chain_str).await {
            Ok(Some(info)) => {
                let chain_id = info.chain().unwrap_or_default();
                let parts: Vec<&str> = chain_id.split(':').collect();
                let (chain_type, chain_id_str) = if parts.len() == 2 {
                    (parts[0].to_string(), parts[1].to_string())
                } else {
                    (chain_id.clone(), String::new())
                };

                (
                    Some(BridgeTokenInfo {
                        asset_name: info.asset_name.clone(),
                        chain_type,
                        chain_id: chain_id_str,
                        decimals: info.decimals,
                        defuse_asset_identifier: info.defuse_asset_identifier.clone(),
                    }),
                    Some(info.decimals),
                    Some(info.near_token_id.clone()),
                )
            }
            Ok(None) => (None, None, None),
            Err(e) => {
                info!(
                    error = %e,
                    "Bridge API not available, using fallback"
                );
                (None, None, None)
            }
        };

    // Use OMFT token ID from bridge API if available, otherwise resolve locally
    let omft_token_id = if let Some(omft_id) = bridge_omft_id {
        omft_id
    } else {
        match app
            .token_registry
            .resolve_to_omft(&query.asset, &chain_str)
            .await
        {
            Ok(omft_id) => omft_id,
            Err(e) => {
                error!(
                    asset = %query.asset,
                    chain = %chain_str,
                    error = %e,
                    "Failed to resolve OMFT token ID"
                );
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": format!("Unknown asset '{}' for chain {}: {}", query.asset, chain_str, e)
                    })),
                )
                    .into_response();
            }
        }
    };

    // Get decimals if not already available
    let final_decimals = match decimals {
        Some(d) => Some(d),
        None => {
            match app
                .token_registry
                .get_decimals(&query.asset, &chain_str)
                .await
            {
                Ok(d) => Some(d),
                Err(_) => None,
            }
        }
    };

    info!(
        omft_token_id = %omft_token_id,
        decimals = ?final_decimals,
        "Token lookup successful"
    );

    (
        StatusCode::OK,
        Json(TokenLookupResponse {
            asset: query.asset,
            chain: chain_str,
            omft_token_id,
            decimals: final_decimals,
            bridge_info,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bridge::BridgeClient, chain::NearHandler};
    use near_crypto::{KeyType, SecretKey};
    use near_primitives::types::AccountId;
    use std::{str::FromStr, sync::Arc};

    fn create_test_app() -> App {
        let bridge_client = Arc::new(BridgeClient::new("https://test.api".to_string()));
        let token_registry = crate::tokens::TokenRegistry::new(Arc::clone(&bridge_client));

        let near_handler = Arc::new(NearHandler::new(
            AccountId::from_str("test.near").unwrap(),
            SecretKey::from_random(KeyType::ED25519),
            "https://rpc.testnet.near.org".to_string(),
            0,
            true,
        ));

        App {
            near_handler,
            bridge_client,
            token_registry,
            tracker: crate::tracker::OperationTracker::new(),
            external_chains: std::sync::Arc::new(
                crate::external::ExternalChainRegistry::new(),
            ),
            dry_run: false,
            version: "0.1.0-test",
        }
    }

    #[tokio::test]
    async fn test_token_lookup_known_asset() {
        let app = create_test_app();

        let query = TokenLookupQuery {
            asset: "USDT".to_string(),
            chain: "ethereum".to_string(),
        };

        let response = token_lookup(State(app), Query(query)).await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_token_lookup_native_eth() {
        let app = create_test_app();

        let query = TokenLookupQuery {
            asset: "ETH".to_string(),
            chain: "eth:1".to_string(),
        };

        let response = token_lookup(State(app), Query(query)).await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_token_lookup_unknown_asset() {
        let app = create_test_app();

        let query = TokenLookupQuery {
            asset: "UNKNOWN".to_string(),
            chain: "eth:1".to_string(),
        };

        let response = token_lookup(State(app), Query(query)).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
