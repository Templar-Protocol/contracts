//! Application state and initialization

use std::sync::Arc;

use crate::{
    bridge::BridgeClient,
    bridge_transport::{BridgeRelayer, HotBridgeRelayer},
    config::Args,
    external::{config::EvmChainConfig, evm::EvmChainHandler, ExternalChainRegistry},
    hot_relayer::{HotMpcApiClient, HotRelayerRouting},
    tokens::TokenRegistry,
    treasury::NearHandler,
    VERSION,
};

/// Application state shared across all request handlers
#[derive(Clone)]
pub struct App {
    /// NEAR handler for treasury operations
    pub near_handler: Arc<NearHandler>,

    /// Bridge API client for cross-chain operations
    pub bridge_client: Arc<BridgeClient>,

    /// Token registry for decimal handling and token info
    pub token_registry: TokenRegistry,

    /// External chain registry for cross-chain deposits
    pub external_chains: Arc<ExternalChainRegistry>,

    /// Optional bridge relayer backend used for transport-specific completion flows.
    pub bridge_relayer: Option<Arc<dyn BridgeRelayer + Send + Sync>>,

    /// Optional bearer token required for relay completion endpoints.
    pub bridge_relayer_auth_token: Option<String>,

    /// Configuration
    pub config: Arc<Args>,

    /// Dry run mode
    pub dry_run: bool,

    /// Application version
    pub version: &'static str,
}

impl App {
    /// Create new application instance from configuration
    pub fn new(args: &Args) -> Self {
        let bridge_client = Arc::new(BridgeClient::new(args.bridge_api_url.clone()));

        // NEAR treasury handler is required
        let near_handler = {
            let account = args
                .near_treasury_account
                .as_ref()
                .expect("NEAR treasury account required");
            let key = args
                .near_treasury_key
                .as_ref()
                .expect("NEAR treasury key required");

            tracing::info!(
                account = %account,
                "Initializing NEAR treasury handler"
            );

            Arc::new(NearHandler::new(
                account.clone(),
                key.clone(),
                args.get_near_treasury_rpc_url(),
                args.dry_run,
            ))
        };

        let token_registry = TokenRegistry::new(Arc::clone(&bridge_client));

        // Initialize external chain registry
        let external_chains = Self::build_external_chain_registry(args);
        let bridge_relayer = Self::build_bridge_relayer();
        let bridge_relayer_auth_token = Self::build_bridge_relayer_auth_token();

        Self {
            near_handler,
            bridge_client,
            token_registry,
            external_chains,
            bridge_relayer,
            bridge_relayer_auth_token,
            config: Arc::new(args.clone()),
            dry_run: args.dry_run,
            version: VERSION,
        }
    }

    /// Build optional bridge relayer backend from environment variables.
    ///
    /// `BRIDGE_RELAYER_BACKEND` options:
    /// - `none` (default)
    /// - `hot`
    ///
    /// For `hot`, these environment variables are required:
    /// - `HOT_MPC_API_URL`
    /// - `HOT_RELAYER_NEAR_RECEIVER`
    /// - `HOT_RELAYER_STELLAR_RECEIVER`
    fn build_bridge_relayer() -> Option<Arc<dyn BridgeRelayer + Send + Sync>> {
        let backend = std::env::var("BRIDGE_RELAYER_BACKEND")
            .unwrap_or_else(|_| "none".to_string())
            .to_lowercase();

        match backend.as_str() {
            "" | "none" => None,
            "hot" => {
                let mpc_api_url = match std::env::var("HOT_MPC_API_URL") {
                    Ok(v) if !v.trim().is_empty() => v,
                    _ => {
                        tracing::warn!(
                            "BRIDGE_RELAYER_BACKEND=hot but HOT_MPC_API_URL is missing; disabling bridge relayer backend"
                        );
                        return None;
                    }
                };
                let near_receiver = match std::env::var("HOT_RELAYER_NEAR_RECEIVER") {
                    Ok(v) if !v.trim().is_empty() => v,
                    _ => {
                        tracing::warn!(
                            "BRIDGE_RELAYER_BACKEND=hot but HOT_RELAYER_NEAR_RECEIVER is missing; disabling bridge relayer backend"
                        );
                        return None;
                    }
                };
                let stellar_receiver = match std::env::var("HOT_RELAYER_STELLAR_RECEIVER") {
                    Ok(v) if !v.trim().is_empty() => v,
                    _ => {
                        tracing::warn!(
                            "BRIDGE_RELAYER_BACKEND=hot but HOT_RELAYER_STELLAR_RECEIVER is missing; disabling bridge relayer backend"
                        );
                        return None;
                    }
                };

                let routing = HotRelayerRouting {
                    near_receiver,
                    stellar_receiver,
                };
                let signer = HotMpcApiClient::new(mpc_api_url);
                let relayer = HotBridgeRelayer::new(routing, signer);

                tracing::info!(backend = "hot", "Configured bridge relayer backend");
                Some(Arc::new(relayer))
            }
            other => {
                tracing::warn!(
                    backend = %other,
                    "Unknown BRIDGE_RELAYER_BACKEND; disabling bridge relayer backend"
                );
                None
            }
        }
    }

    fn build_bridge_relayer_auth_token() -> Option<String> {
        match std::env::var("BRIDGE_RELAYER_AUTH_TOKEN") {
            Ok(token) => {
                let trimmed = token.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            Err(_) => None,
        }
    }

    /// Build external chain registry from configuration
    fn build_external_chain_registry(args: &Args) -> Arc<ExternalChainRegistry> {
        let mut registry = ExternalChainRegistry::new();

        // If ETH private key is configured, register all EVM chains
        if let Some(ref private_key) = args.eth_private_key {
            tracing::info!("Configuring EVM chains for automated deposits");

            // Register all default EVM chains
            for config in EvmChainConfig::all_defaults() {
                let chain_name = config.name.clone();
                let chain_id = config.chain_id.clone();

                // Override RPC URL if configured
                let config =
                    if chain_id == "eth:1" && args.eth_rpc_url != "https://eth.llamarpc.com" {
                        let mut c = config;
                        c.rpc_url = args.eth_rpc_url.clone();
                        c
                    } else {
                        config
                    };

                let handler = EvmChainHandler::new(config, private_key.clone());
                registry.register(Box::new(handler));

                tracing::info!(
                    chain = %chain_name,
                    chain_id = %chain_id,
                    "Registered EVM chain handler"
                );
            }
        } else {
            tracing::info!("ETH_PRIVATE_KEY not configured - EVM deposits disabled");
        }

        // Check for Solana configuration from environment
        if let Some(solana_handler) = crate::external::solana::solana_sdk_handler_from_env() {
            let chain_id = solana_handler.chain_id().to_string();
            tracing::info!(
                chain_id = %chain_id,
                "Registered Solana chain handler"
            );
            registry.register(solana_handler);
        } else {
            tracing::info!("No Solana keypair configured - Solana deposits disabled");
        }

        // Check for Stellar configuration from environment
        if let Some(stellar_handler) = crate::external::stellar::stellar_handler_from_env() {
            let chain_id = stellar_handler.chain_id().to_string();
            tracing::info!(
                chain_id = %chain_id,
                "Registered Stellar chain handler"
            );
            registry.register(stellar_handler);
        } else {
            tracing::info!("No Stellar keypair configured - Stellar deposits disabled");
        }

        // Check for NEAR external configuration from environment
        if let Some(near_handler) = crate::external::near::near_handler_from_env() {
            let chain_id = near_handler.chain_id().to_string();
            tracing::info!(
                chain_id = %chain_id,
                "Registered NEAR external chain handler"
            );
            registry.register(near_handler);
        } else {
            tracing::info!("No NEAR external keypair configured - NEAR deposits disabled");
        }

        Arc::new(registry)
    }

    /// Check if service is healthy
    pub fn is_healthy(&self) -> bool {
        true // NEAR handler is always available if configured
    }

    /// Get list of available external chains
    pub fn available_external_chains(&self) -> Vec<String> {
        self.external_chains.chains()
    }
}
