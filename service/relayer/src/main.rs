use std::net::SocketAddr;

use axum::{routing, Router};
use clap::Parser;
use tokio::{signal, sync::watch, task::JoinSet};
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use templar_relayer::{
    app::{App, Configuration},
    route,
};

#[allow(clippy::unwrap_used)]
#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true),
        )
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Configuration::parse();
    let kill = watch::Sender::default();

    let mut app = App::new(args, kill.clone());
    app.database.migrate().await.unwrap();
    app.load_markets().await;

    let addr = SocketAddr::from(([0, 0, 0, 0], app.args.port));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    let router = Router::new()
        .route("/", routing::get(|| async { "Hello, World!" }))
        .route("/relay", routing::post(route::relay::relay))
        .route(
            "/get_allowance",
            routing::get(route::get_allowance::get_allowance),
        )
        .nest("/universal_account", route::universal_account::router())
        .layer(ServiceBuilder::new().layer(CorsLayer::permissive()))
        .with_state(app);

    tracing::info!("Listening on {}", listener.local_addr().unwrap());

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(kill))
        .await
        .unwrap();
}

// https://github.com/tokio-rs/axum/blob/9ec85d69703a9065a1098bb43bd93113695d5ade/examples/graceful-shutdown/src/main.rs
#[allow(clippy::expect_used, clippy::unwrap_used)]
async fn shutdown_signal(kill: watch::Sender<()>) {
    let mut on_kill = kill.subscribe();

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    let mut signals = [
        ("SIGINT", signal::unix::SignalKind::interrupt()),
        ("SIGTERM", signal::unix::SignalKind::terminate()),
        ("SIGQUIT", signal::unix::SignalKind::quit()),
    ]
    .into_iter()
    .map(|(name, sig)| async move {
        signal::unix::signal(sig)
            .expect("Failed to install signal handler")
            .recv()
            .await;
        name
    })
    .collect::<JoinSet<_>>();

    tokio::select! {
        _ = on_kill.changed() => {
            tracing::debug!("Received kill notification");
        },
        () = ctrl_c => {
            tracing::info!("Received Ctrl+C");
            kill.send(()).unwrap();
        },
        Some(Ok(s)) = signals.join_next() => {
            tracing::info!("Received {s}");
            kill.send(()).unwrap();
        },
    };

    tracing::info!("Exiting");
}
