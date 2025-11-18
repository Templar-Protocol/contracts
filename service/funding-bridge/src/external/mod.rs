//! External chain handlers for cross-chain deposits
//!
//! This module provides abstractions for interacting with external blockchains
//! (Ethereum, Solana, etc.) to transfer tokens to bridge deposit addresses.

pub mod config;
pub mod evm;
#[cfg(feature = "solana")]
pub mod solana;

use async_trait::async_trait;
use std::collections::HashMap;

/// Result of a transfer operation
#[derive(Debug, Clone)]
pub struct TransferResult {
    /// Transaction hash on the source chain
    pub tx_hash: String,
    /// Whether the transaction is confirmed
    pub confirmed: bool,
}

/// Error from external chain operations
#[derive(Debug, thiserror::Error)]
pub enum ExternalChainError {
    #[error("Chain not configured: {0}")]
    NotConfigured(String),

    #[error("Invalid private key: {0}")]
    InvalidPrivateKey(String),

    #[error("RPC connection failed: {0}")]
    RpcConnectionFailed(String),

    #[error("Token not supported: {asset} on {chain}")]
    TokenNotSupported { asset: String, chain: String },

    #[error("Invalid address: {0}")]
    InvalidAddress(String),

    #[error("Invalid amount: {0}")]
    InvalidAmount(String),

    #[error("Transaction failed: {0}")]
    TransactionFailed(String),

    #[error("Feature not enabled: {0}")]
    FeatureNotEnabled(String),
}

/// Trait for external chain handlers
#[async_trait]
pub trait ExternalChainHandler: Send + Sync {
    /// Get the chain identifier (e.g., "eth:1", "eth:42161")
    fn chain_id(&self) -> &str;

    /// Check if this handler supports the given token
    fn supports_token(&self, asset: &str) -> bool;

    /// Transfer tokens to the given address
    ///
    /// # Arguments
    /// * `to_address` - Destination address (bridge deposit address)
    /// * `asset` - Token symbol (e.g., "USDC", "USDT")
    /// * `amount` - Human-readable amount (e.g., "100.5")
    async fn transfer_tokens(
        &self,
        to_address: &str,
        asset: &str,
        amount: &str,
    ) -> Result<TransferResult, ExternalChainError>;
}

/// Registry of external chain handlers
pub struct ExternalChainRegistry {
    handlers: HashMap<String, Box<dyn ExternalChainHandler>>,
}

impl ExternalChainRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a chain handler
    pub fn register(&mut self, handler: Box<dyn ExternalChainHandler>) {
        let chain_id = handler.chain_id().to_string();
        self.handlers.insert(chain_id, handler);
    }

    /// Get a handler for the given chain
    pub fn get(&self, chain_id: &str) -> Option<&dyn ExternalChainHandler> {
        self.handlers.get(chain_id).map(|h| h.as_ref())
    }

    /// Check if a chain is registered
    pub fn has_chain(&self, chain_id: &str) -> bool {
        self.handlers.contains_key(chain_id)
    }

    /// Get all registered chain IDs
    pub fn chains(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }
}

impl Default for ExternalChainRegistry {
    fn default() -> Self {
        Self::new()
    }
}
