mod config;
mod logging;
mod rpc;

use std::collections::BTreeMap;

use clap::Parser;
use jsonrpsee::server::ServerBuilder;
use tokio::signal;

use crate::config::Config;

async fn build_signers(
    config: &Config,
) -> BTreeMap<near_account_id::AccountId, std::sync::Arc<near_api::Signer>> {
    let mut signers = BTreeMap::new();

    for managed_signer in &config.managed_signers {
        let mut secret_keys = managed_signer.secret_keys.iter().cloned();
        let first_secret_key = secret_keys
            .next()
            .expect("managed signer config should contain at least one secret key");
        let signer = near_api::Signer::from_secret_key(first_secret_key)
            .expect("failed to create signer from secret key");

        for secret_key in secret_keys {
            signer
                .add_secret_key_to_pool(secret_key)
                .await
                .expect("failed to add secret key to signer pool");
        }

        signers.insert(managed_signer.account_id.clone(), signer);
    }

    signers
}

#[allow(clippy::expect_used, reason = "fail fast during startup/shutdown")]
#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let config = Config::parse();
    let _log_guard = logging::init();

    let network = near_api::NetworkConfig::from_rpc_url("gateway", config.near_rpc_url.clone());
    let near = blockchain_gateway_near::NearReadClient::new(network.clone());
    let writer =
        blockchain_gateway_near::NearWriteClient::new(network, build_signers(&config).await);
    let gateway = blockchain_gateway_near::GatewayService::new(near, writer);

    let server = ServerBuilder::default()
        .build(config.listen_addr)
        .await
        .expect("failed to bind blockchain gateway server");
    let local_addr = server
        .local_addr()
        .expect("server should have a bound local address");
    let module = rpc::attach_gateway(gateway).expect("failed to attach RPC methods");
    let handle = server.start(module);

    tracing::info!(%local_addr, "blockchain gateway service listening");

    shutdown_signal().await;
    handle
        .stop()
        .expect("blockchain gateway server should stop cleanly");
    handle.stopped().await;
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
