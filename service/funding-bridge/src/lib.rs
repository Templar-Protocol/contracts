//! # Templar Funding Bridge
//!
//! NEAR-centric treasury management service for cross-chain operations.
//! Integrates with NEAR Intents Bridge API to enable cross-chain deposits and withdrawals.
//!
//! ## Features
//!
//! - **Cross-chain withdrawals** via NEAR Intents (NEP-413 signed intents)
//! - **Deposit address generation** for external chains (Ethereum, Arbitrum, Solana, etc.)
//! - **Token registry** with automatic OMFT token ID resolution
//! - **NEAR treasury management** - holds OMFT tokens, withdraws to any chain
//! - **Stateless design** - no database, all state on-chain
//!
//! ## API Endpoints
//!
//! - `GET /health` - Service health check
//! - `GET /metrics` - Prometheus-compatible metrics
//! - `GET /tokens/lookup?asset=USDT&chain=ethereum` - Resolve OMFT token IDs
//! - `POST /deposit` - **Automated deposit from external wallet** (ETH/Arbitrum)
//! - `POST /withdraw` - Initiate cross-chain withdrawal via NEAR Intents
//!
//! ## Quick Start
//!
//! ```bash
//! # Start the service (dry-run mode)
//! funding-bridge \
//!   --network mainnet \
//!   --near-account your-treasury.near \
//!   --near-signer-key "ed25519:..." \
//!   --port 3000 \
//!   --dry-run
//! ```
//!
//! ## Module Organization
//!
//! - [`app`] - Application state and initialization
//! - [`bridge`] - NEAR Intents Bridge API client
//! - [`chain`] - NEAR treasury handler
//! - [`config`] - CLI arguments and configuration
//! - [`error`] - Error types and handling
//! - [`intents`] - NEAR Intents protocol (NEP-413 signing)
//! - [`metrics`] - Prometheus metrics collection
//! - [`routes`] - REST API endpoints
//! - [`rpc`] - NEAR network configuration
//! - [`tokens`] - Token registry and OMFT utilities

pub mod app;
pub mod bridge;
pub mod chain;
pub mod config;
pub mod error;
pub mod external;
pub mod intents;
pub mod metrics;
pub mod routes;
pub mod rpc;
pub mod tokens;

// Re-export commonly used types
pub use config::Args;
pub use error::{FundingError, FundingResult};

/// Service version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
