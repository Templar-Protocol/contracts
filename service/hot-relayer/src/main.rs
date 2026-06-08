use std::net::SocketAddr;

use axum::extract::DefaultBodyLimit;
use clap::Parser;
use templar_hot_relayer::{routes, Config, VERSION};
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hot_relayer=debug,tower_http=debug,axum=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli_config = Config::parse();
    let config = match cli_config.validate() {
        Ok(config) => config,
        Err(error) => {
            error!(%error, "invalid HOT relayer configuration");
            return Err(error.into());
        }
    };
    let state = match routes::AppState::from_validated_config(&config) {
        Ok(state) => state,
        Err(error) => {
            error!(%error, "invalid HOT relayer configuration");
            return Err(error.into());
        }
    };

    info!(
        version = VERSION,
        port = config.port(),
        chain_id = config.routing().chain_id(),
        near_receiver = %config.routing().near_receiver(),
        token_id = %config.routing().token_id(),
        mpc_api_url = %config.hot_mpc_api_url().redacted(),
        mpc_timeout_secs = config.mpc_timeout().seconds(),
        max_request_bytes = config.max_request_bytes().get(),
        "starting HOT relayer"
    );

    let router = routes::router(state).layer(
        ServiceBuilder::new()
            .layer(DefaultBodyLimit::max(config.max_request_bytes().get()))
            .layer(TraceLayer::new_for_http()),
    );
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port()));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(addr = %listener.local_addr()?, "listening for HOT relay requests");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(error) => {
                error!(%error, "failed to install SIGTERM handler");
                await_ctrl_c().await;
                return;
            }
        };

        tokio::select! {
            () = await_ctrl_c() => {},
            _ = terminate.recv() => {
                info!("SIGTERM received");
            },
        }
    }

    #[cfg(not(unix))]
    {
        await_ctrl_c().await;
    }

    info!("shutdown signal received");
}

async fn await_ctrl_c() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        error!(%error, "failed to install Ctrl-C handler");
    }
}
