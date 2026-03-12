//! Funding Bridge Service Entry Point
//!
//! Main entry point for the funding-bridge service that starts the REST API server.

use std::net::SocketAddr;

use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use templar_funding_bridge::{Args, FundingResult, VERSION};

#[tokio::main]
async fn main() -> FundingResult<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "funding_bridge=debug,tower_http=debug,axum=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Parse CLI arguments
    let args = Args::parse_args()?;

    info!(
        version = VERSION,
        port = args.port,
        network = ?args.network,
        dry_run = args.dry_run,
        "Starting Funding Bridge Service"
    );

    // Validate configuration
    if let Err(e) = args.validate() {
        error!(error = %e, "Configuration validation failed");
        return Err(e);
    }

    info!("Configuration validated successfully");

    // Initialize application state
    let app = templar_funding_bridge::app::App::new(&args);

    if !app.is_healthy() {
        error!("No chain handlers available - service cannot start");
        return Err(templar_funding_bridge::error::FundingError::ConfigError(
            "No chain handlers available".to_string(),
        ));
    }

    info!(
        treasury = %app.near_handler.treasury_account(),
        "Application initialized with NEAR treasury"
    );

    // Create router with all routes
    let router = templar_funding_bridge::routes::create_router()
        .layer(ServiceBuilder::new().layer(CorsLayer::permissive()))
        .with_state(app);

    // Bind to address
    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        error!(error = %e, "Failed to bind to address");
        templar_funding_bridge::error::FundingError::Internal(format!(
            "Failed to bind to {}: {}",
            addr, e
        ))
    })?;

    info!(addr = %listener.local_addr().unwrap(), "Listening for requests");
    info!("Service initialized, ready to serve requests");

    // Start server with graceful shutdown
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| {
            templar_funding_bridge::error::FundingError::Internal(format!("Server error: {}", e))
        })?;

    info!("Shutdown complete");

    Ok(())
}

/// Wait for shutdown signal (Ctrl+C)
async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install Ctrl+C handler");

    info!("Shutdown signal received, stopping service");
}
