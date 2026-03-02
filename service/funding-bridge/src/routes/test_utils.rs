//! Test utilities for route handlers

#[cfg(test)]
use {
    crate::{app::App, bridge::BridgeClient, config::Args, rpc::Network, treasury::NearHandler},
    near_crypto::{KeyType, SecretKey},
    near_primitives::types::AccountId,
    std::{str::FromStr, sync::Arc},
};

#[cfg(test)]
pub fn create_test_app() -> App {
    let args = Args {
        port: 3000,
        network: Network::Mainnet,
        bridge_api_url: "https://test.api".to_string(),
        dry_run: false,
        near_treasury_account: Some(AccountId::from_str("test.near").unwrap()),
        near_treasury_key: Some(SecretKey::from_random(KeyType::ED25519)),
        near_rpc_url: None,
        eth_private_key: None,
        eth_rpc_url: "https://eth.llamarpc.com".to_string(),
        solana_private_key: None,
        solana_rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
        eth_withdraw_address: Some("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string()),
        arbitrum_withdraw_address: Some("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string()),
        base_withdraw_address: Some("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string()),
        optimism_withdraw_address: None,
        polygon_withdraw_address: None,
        solana_withdraw_address: Some("B4b13ZjqPNGmvK7VVXM3kZ3vEpKS7JVzuqVU6vGqXm9D".to_string()),
        stellar_secret_key: None,
        stellar_horizon_url: "https://horizon.stellar.org".to_string(),
        stellar_withdraw_address: None,
    };

    let bridge_client = Arc::new(BridgeClient::new(args.bridge_api_url.clone()));
    let token_registry = crate::tokens::TokenRegistry::new(Arc::clone(&bridge_client));

    let near_handler = Arc::new(NearHandler::new(
        args.near_treasury_account.clone().unwrap(),
        args.near_treasury_key.clone().unwrap(),
        args.get_near_treasury_rpc_url(),
        true,
    ));

    App {
        near_handler,
        bridge_client,
        token_registry,
        external_chains: std::sync::Arc::new(crate::external::ExternalChainRegistry::new()),
        bridge_relayer: None,
        bridge_relayer_auth_token: None,
        config: Arc::new(args),
        dry_run: false,
        version: "0.1.0-test",
    }
}
