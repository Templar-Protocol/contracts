use std::net::SocketAddr;

use clap::Parser;
use url::Url;

#[derive(Debug, Clone, Parser)]
pub struct Config {
    /// TCP address for the blockchain gateway JSON-RPC server.
    #[arg(long, env = "LISTEN_ADDR", default_value = "127.0.0.1:9944")]
    pub listen_addr: SocketAddr,

    /// NEAR RPC endpoint used by the gateway for on-chain reads and writes.
    #[arg(long, env = "NEAR_RPC_URL")]
    pub near_rpc_url: Url,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let config = Config::try_parse_from([
            "blockchain-gateway-service",
            "--near-rpc-url",
            "https://rpc.mainnet.near.org",
            "--listen-addr",
            "1.2.3.4:3333",
        ])
        .expect("config should parse");

        assert_eq!(config.listen_addr, "1.2.3.4:3333".parse().unwrap());
        assert_eq!(
            config.near_rpc_url.as_str(),
            "https://rpc.mainnet.near.org/"
        );
    }
}
