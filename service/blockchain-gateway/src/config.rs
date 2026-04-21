use std::collections::HashMap;
use std::str::FromStr;
use std::{net::SocketAddr, path::PathBuf};

use blockchain_gateway_core::ManagedAccountId;
use blockchain_gateway_near::{ManagedSigner, PostgresStore, SharedOperationStore};
use clap::Parser;
use near_account_id::AccountId;
use near_api::types::SecretKey;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSignerConfig {
    pub account_id: AccountId,
    pub secret_keys: Vec<SecretKey>,
}

impl FromStr for ManagedSignerConfig {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (account_id, secret_keys) = value
            .split_once('=')
            .ok_or_else(|| "expected <account_id>=<secret_key>[,<secret_key>...]".to_owned())?;

        let account_id = account_id
            .parse()
            .map_err(|error| format!("invalid account id: {error}"))?;
        let secret_keys = secret_keys
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse()
                    .map_err(|error| format!("invalid secret key: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        if secret_keys.is_empty() {
            return Err("at least one secret key is required".to_owned());
        }

        Ok(Self {
            account_id,
            secret_keys,
        })
    }
}

#[derive(Debug, Clone, Parser)]
pub struct Config {
    /// TCP address for the blockchain gateway JSON-RPC server.
    #[arg(long, env = "LISTEN_ADDR", default_value = "127.0.0.1:9944")]
    pub listen_addr: SocketAddr,

    /// NEAR RPC endpoint used by the gateway for on-chain reads and writes.
    #[arg(
        long,
        env = "NEAR_RPC_URL",
        default_value = "https://rpc.testnet.near.org"
    )]
    pub near_rpc_url: Url,

    /// Postgres database URL for durable gateway operation storage.
    #[arg(long, env = "GATEWAY_DATABASE_URL")]
    pub database_url: Option<String>,

    /// Run gateway Postgres migrations during startup.
    #[arg(long, env = "GATEWAY_DATABASE_MIGRATE", default_value_t = false)]
    pub migrate_database: bool,

    /// Pyth Hermes API URL used when the gateway needs to fetch fresh update payloads.
    #[arg(
        long,
        env = "PYTH_HERMES_URL",
        default_value = "https://hermes-beta.pyth.network"
    )]
    pub pyth_hermes_url: Url,

    /// Path to the executable used for RedStone bridge payload generation.
    #[arg(long, env = "REDSTONE_NODE_PATH", default_value = "node")]
    pub redstone_node_path: PathBuf,

    /// Managed signer entries as <account_id>=<secret_key>[,<secret_key>...].
    #[arg(
        long = "managed-signer",
        env = "MANAGED_SIGNERS",
        value_delimiter = ';'
    )]
    pub managed_signers: Vec<ManagedSignerConfig>,
}

impl Config {
    pub async fn build_signers(&self) -> HashMap<ManagedAccountId, ManagedSigner> {
        let mut signers = HashMap::new();

        for config in &self.managed_signers {
            let secret_keys = config.secret_keys.iter().cloned();
            let entry = ManagedSigner::new(secret_keys)
                .await
                .expect("failed to initialize signer");
            signers.insert(ManagedAccountId(config.account_id.clone()), entry);
        }

        signers
    }

    pub fn build_store(&self) -> Result<Option<SharedOperationStore>, sqlx::Error> {
        self.database_url
            .as_deref()
            .map(PostgresStore::new)
            .transpose()
            .map(|store| store.map(|store| std::sync::Arc::new(store) as SharedOperationStore))
    }
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
            "--managed-signer",
            "test.near=ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q,ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q",
        ])
        .expect("config should parse");

        assert_eq!(config.listen_addr, "1.2.3.4:3333".parse().unwrap());
        assert_eq!(
            config.near_rpc_url.as_str(),
            "https://rpc.mainnet.near.org/"
        );
        assert_eq!(config.database_url, None);
        assert!(!config.migrate_database);
        assert_eq!(
            config.pyth_hermes_url.as_str(),
            "https://hermes-beta.pyth.network/"
        );
        assert_eq!(config.redstone_node_path, PathBuf::from("node"));
        assert_eq!(config.managed_signers.len(), 1);
        assert_eq!(config.managed_signers[0].account_id.as_str(), "test.near");
        assert_eq!(config.managed_signers[0].secret_keys.len(), 2);
    }
}
