use std::net::SocketAddr;

use clap::Parser;
use url::Url;

#[derive(Debug, Clone, Parser)]
#[command(name = "blockchain-gateway-service")]
pub struct Config {
    /// TCP address for the blockchain gateway JSON-RPC server.
    #[arg(long, env = "LISTEN_ADDR", default_value = "127.0.0.1:9944")]
    pub listen_addr: SocketAddr,

    /// NEAR RPC endpoint used by the gateway for on-chain reads and writes.
    #[arg(long, env = "NEAR_RPC_URL")]
    pub near_rpc_url: Url,
}
