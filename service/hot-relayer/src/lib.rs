//! Standalone HOT bridge relayer service.
//!
//! This crate is intentionally narrow: it validates HOT deposit/withdrawal completion
//! payloads, requires bearer auth, and calls the configured HOT MPC API. It does not
//! load treasury keys or initialize unrelated chain handlers.

pub mod bridge_transport;
pub mod config;
pub mod hot_relayer;
pub mod metrics;
pub mod routes;

pub use config::Config;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
