//! NEAR Intents Bridge API integration
//!
//! This module provides a type-safe wrapper around the NEAR Intents Bridge JSON-RPC API.

pub mod client;
pub mod models;

pub use client::{BridgeClient, MAINNET_BRIDGE_API};
pub use models::*;
