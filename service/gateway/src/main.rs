mod config;
mod gateway_service;
mod logging;
mod rpc;

use crate::rpc::attach_gateway;
use clap::Parser;
use jsonrpsee::server::ServerBuilder;
use near_api::NetworkConfig;
use templar_gateway_core::GatewayContext;
use templar_gateway_oracle_updates_dispatch::GatewayContextBuilderOracleExt;
use tokio::signal;

use crate::config::Config;
use crate::gateway_service::GatewayService;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let config = Config::parse();
    let _log_guard = logging::init();

    let signers = config.build_signers().await?;
    let store = config.build_store().await?;
    let context =
        GatewayContext::builder(NetworkConfig::from_rpc_url("gateway", config.near_rpc_url))
            .with_pyth_source(config.pyth_hermes_url)
            .with_redstone_source(&config.redstone_node_path)
            .map_err(anyhow::Error::from)?
            .build();
    let service = GatewayService::spawn(context, signers, store);

    let server = ServerBuilder::default().build(config.listen_addr).await?;
    let local_addr = server.local_addr()?;
    let module = attach_gateway(service.clone())?;
    let handle = server.start(module);

    tracing::info!(%local_addr, "gateway service listening");

    shutdown_signal().await?;
    handle.stop()?;
    handle.stopped().await;
    service.shutdown().await;
    Ok(())
}

async fn shutdown_signal() -> anyhow::Result<()> {
    let ctrl_c = async { signal::ctrl_c().await };

    #[cfg(unix)]
    {
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())?;
        let mut sigquit = signal::unix::signal(signal::unix::SignalKind::quit())?;

        tokio::select! {
            result = ctrl_c => {
                result?;
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
        ctrl_c.await?;
        tracing::info!("received Ctrl+C");
    }
    Ok(())
}
