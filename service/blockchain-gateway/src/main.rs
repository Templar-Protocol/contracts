mod config;
mod logging;
mod rpc;

use blockchain_gateway_near::{GatewayContext, GatewayService};
use clap::Parser;
use jsonrpsee::server::ServerBuilder;
use near_api::NetworkConfig;
use tokio::signal;

use crate::config::Config;

#[allow(clippy::expect_used, reason = "fail fast during startup/shutdown")]
#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let config = Config::parse();
    let _log_guard = logging::init();

    let signers = config.build_signers().await;
    let context = GatewayContext::new(
        NetworkConfig::from_rpc_url("gateway", config.near_rpc_url),
        config.pyth_hermes_url,
        &config.redstone_node_path,
    )
    .expect("failed to initialize gateway context");
    let gateway = GatewayService::spawn(context, signers);

    let server = ServerBuilder::default()
        .build(config.listen_addr)
        .await
        .expect("failed to bind blockchain gateway server");
    let local_addr = server
        .local_addr()
        .expect("server should have a bound local address");
    let module = rpc::attach_gateway(gateway.clone()).expect("failed to attach RPC methods");
    let handle = server.start(module);

    tracing::info!(%local_addr, "blockchain gateway service listening");

    shutdown_signal().await;
    handle
        .stop()
        .expect("blockchain gateway server should stop cleanly");
    handle.stopped().await;
    gateway.shutdown().await;
}

#[allow(clippy::expect_used, reason = "fail fast")]
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    {
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        let mut sigquit = signal::unix::signal(signal::unix::SignalKind::quit())
            .expect("failed to install SIGQUIT handler");

        tokio::select! {
            () = ctrl_c => {
                tracing::info!("received Ctrl+C");
            }
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM");
            }
            _ = sigquit.recv() => {
                tracing::info!("received SIGQUIT");
            }
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await;
        tracing::info!("received Ctrl+C");
    }
}
