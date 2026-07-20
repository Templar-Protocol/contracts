//! Application state and initialization

use std::sync::Arc;

use crate::{
    bridge::BridgeClient,
    config::Args,
    error::{FundingError, FundingResult},
    external::{config::EvmChainConfig, evm::EvmChainHandler, ExternalChainRegistry},
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

    /// Configuration
    pub config: Arc<Args>,

    /// Dry run mode
    pub dry_run: bool,

    /// Application version
    pub version: &'static str,
}

impl App {
    /// Create new application instance from configuration.
    ///
    /// Returns a [`FundingError::ConfigError`] when required treasury settings
    /// are missing, or a [`FundingError::ChainError`] when the treasury handler
    /// cannot be built (e.g. a malformed `NEAR_RPC_URL`) — startup config
    /// problems surface to `main` rather than panicking.
    pub fn new(args: &Args) -> FundingResult<Self> {
        let bridge_client = Arc::new(BridgeClient::new(args.bridge_api_url.clone()));

        // NEAR treasury handler is required
        let near_handler = {
            let account = args.near_treasury_account.as_ref().ok_or_else(|| {
                FundingError::ConfigError("NEAR treasury account required".to_string())
            })?;
            let key = args.near_treasury_key.as_ref().ok_or_else(|| {
                FundingError::ConfigError("NEAR treasury key required".to_string())
            })?;

            tracing::info!(
                account = %account,
                "Initializing NEAR treasury handler"
            );

            Arc::new(
                NearHandler::new(
                    account.clone(),
                    key.clone(),
                    args.get_near_treasury_rpc_url(),
                    args.network,
                    args.dry_run,
                )
                .map_err(|source| FundingError::ChainError {
                    chain: "near".to_string(),
                    source,
                })?,
            )
        };

        let token_registry = TokenRegistry::new(Arc::clone(&bridge_client));

        // Initialize external chain registry
        let external_chains = Self::build_external_chain_registry(args);

        Ok(Self {
            near_handler,
            bridge_client,
            token_registry,
            external_chains,
            config: Arc::new(args.clone()),
            dry_run: args.dry_run,
            version: VERSION,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `App::new` builds the NEAR handler from config without any network I/O —
    /// only a syntactically valid RPC URL and a configured treasury are needed.
    #[test]
    fn app_new_builds_configured_treasury_handler() {
        let args = Args::test_valid();
        let expected_treasury = args.near_treasury_account.clone().unwrap();

        let app = App::new(&args).expect("build app");

        assert!(app.is_healthy());
        assert_eq!(app.near_handler.treasury_account(), &expected_treasury);
    }

    /// A missing treasury account is a configuration error, not a panic.
    #[test]
    fn app_new_requires_treasury_account() {
        let mut args = Args::test_valid();
        args.near_treasury_account = None;

        assert!(matches!(App::new(&args), Err(FundingError::ConfigError(_))));
    }
}
