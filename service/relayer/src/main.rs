#![allow(clippy::unwrap_used)]

use std::{fs::File, net::SocketAddr};

use axum::{routing, Router};
use clap::Parser;
use tokio::signal;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use templar_relayer::{
    app::{App, Args, Configuration},
    client::database::Database,
};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let configuration: Configuration =
        serde_yaml::from_reader(File::open(&args.config).unwrap()).unwrap();

    let mut app = App::new(args, configuration);
    app.database.migrate().await.unwrap();
    app.load_markets().await;

    let database = app.database.clone();

    let addr = SocketAddr::from(([0, 0, 0, 0], app.args.port));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    let router = Router::new()
        .route("/", routing::get(|| async { "Hello, World!" }))
        .route(
            "/relay",
            routing::post(templar_relayer::route::relay::relay),
        )
        .route(
            "/get_allowance",
            routing::get(templar_relayer::route::get_allowance::get_allowance),
        )
        .with_state(app);

    tracing::info!("Listening on {}", listener.local_addr().unwrap());

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(database))
        .await
        .unwrap();
}

// https://github.com/tokio-rs/axum/blob/9ec85d69703a9065a1098bb43bd93113695d5ade/examples/graceful-shutdown/src/main.rs
#[allow(clippy::expect_used)]
async fn shutdown_signal(database: Database) {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    };

    database.close().await;
}
