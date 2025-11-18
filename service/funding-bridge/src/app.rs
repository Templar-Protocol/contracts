//! Application state and initialization

use std::sync::Arc;

use crate::{
    bridge::BridgeClient,
    chain::NearHandler,
    config::Args,
    external::{config::EvmChainConfig, evm::EvmChainHandler, ExternalChainRegistry},
    tokens::TokenRegistry,
    tracker::OperationTracker,
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

    /// Operation tracker for status queries
    pub tracker: OperationTracker,

    /// External chain registry for cross-chain deposits
    pub external_chains: Arc<ExternalChainRegistry>,

    /// Dry run mode
    pub dry_run: bool,

    /// Application version
    pub version: &'static str,
}

impl App {
    /// Create new application instance from configuration
    pub fn new(args: &Args) -> Self {
        let bridge_client = Arc::new(BridgeClient::new(args.bridge_api_url.clone()));

        // NEAR handler is required
        let near_handler = {
            let account = args
                .near_treasury_account
                .as_ref()
                .expect("NEAR treasury account required");
            let key = args
                .near_signer_key
                .as_ref()
                .expect("NEAR signer key required");

            tracing::info!(
                account = %account,
                "Initializing NEAR handler"
            );

            Arc::new(NearHandler::new(
                account.clone(),
                key.clone(),
                args.get_near_rpc_url(),
                0, // Priority not used
                args.dry_run,
            ))
        };

        let token_registry = TokenRegistry::new(Arc::clone(&bridge_client));
        let tracker = OperationTracker::new();

        // Initialize external chain registry
        let external_chains = Self::build_external_chain_registry(args);

        Self {
            near_handler,
            bridge_client,
            token_registry,
            tracker,
            external_chains,
            dry_run: args.dry_run,
            version: VERSION,
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
                let config = if chain_id == "eth:1" && args.eth_rpc_url != "https://eth.llamarpc.com"
                {
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
            tracing::warn!("ETH_PRIVATE_KEY not configured - EVM deposits disabled");
        }

        // Check for Solana configuration from environment
        #[cfg(feature = "solana")]
        {
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
        }

        #[cfg(not(feature = "solana"))]
        {
            tracing::info!("Solana feature not enabled - Solana deposits disabled");
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
