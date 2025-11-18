//! REST API routes and models

pub mod deposit;
pub mod health;
pub mod metrics;
pub mod models;
pub mod status;
pub mod tokens;
pub mod withdraw;

use axum::{routing, Router};

use crate::app::App;

/// Create router with all routes
pub fn create_router() -> Router<App> {
    Router::new()
        .route("/", routing::get(|| async { "Funding Bridge API v0.1.0" }))
        .route("/health", routing::get(health::health))
        .route("/metrics", routing::get(metrics::metrics))
        .route("/deposit", routing::post(deposit::deposit))
        .route("/withdraw", routing::post(withdraw::withdraw))
        .route("/tokens/lookup", routing::get(tokens::token_lookup))
        .route("/status/:request_id", routing::get(status::get_status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_router_creation() {
        let _router = create_router();
    }
}
