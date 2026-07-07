use std::collections::HashMap;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::Parser;
use near_account_id::AccountId;
use near_api::types::SecretKey;
use templar_gateway_core::{RedactedString, SharedOperationStore};
use templar_gateway_oracle_updates_dispatch::OracleSourceArgs;
use templar_gateway_runtime::ManagedSigner;
use templar_gateway_store::{MemoryStore, PostgresStore};
use templar_gateway_types::ManagedAccountId;
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
    /// TCP address for the Templar Gateway JSON-RPC server.
    #[arg(long, env = "LISTEN_ADDR", default_value = "127.0.0.1:9944")]
    pub listen_addr: SocketAddr,

    /// NEAR RPC endpoint used by the gateway for on-chain reads and writes.
    #[arg(
        long,
        env = "NEAR_RPC_URL",
        default_value = "https://rpc.testnet.near.org"
    )]
    pub near_rpc_url: Url,

    /// API key for the RPC endpoint, sent as an `Authorization` header. May also
    /// be supplied as an `apiKey` query parameter on `--near-rpc-url`.
    #[arg(long, env = "NEAR_RPC_API_KEY")]
    pub near_rpc_api_key: Option<RedactedString>,

    /// Postgres database URL for durable gateway operation storage.
    #[arg(long, env = "GATEWAY_DATABASE_URL")]
    pub database_url: Option<String>,

    /// Run gateway Postgres migrations during startup.
    #[arg(long, env = "GATEWAY_DATABASE_MIGRATE", default_value_t = false)]
    pub migrate_database: bool,

    /// In-process oracle payload sources (Pyth Hermes, RedStone bridge, Pyth Pro/Lazer).
    #[command(flatten)]
    pub oracle_sources: OracleSourceArgs,

    /// Managed signer entries as `<account_id>=<secret_key>[,<secret_key>...]`.
    #[arg(
        long = "managed-signer",
        env = "MANAGED_SIGNERS",
        value_delimiter = ';'
    )]
    pub managed_signers: Vec<ManagedSignerConfig>,
}

impl Config {
    pub async fn build_signers(&self) -> Result<HashMap<ManagedAccountId, ManagedSigner>> {
        let mut signers = HashMap::new();

        for config in &self.managed_signers {
            let secret_keys = config.secret_keys.iter().cloned();
            let entry = ManagedSigner::new(secret_keys).await.with_context(|| {
                format!("failed to initialize signer for {}", config.account_id)
            })?;
            signers.insert(ManagedAccountId(config.account_id.clone()), entry);
        }

        Ok(signers)
    }

    pub async fn build_store(&self) -> Result<SharedOperationStore> {
        let Some(database_url) = self.database_url.as_deref() else {
            if self.migrate_database {
                bail!("--migrate-database requires GATEWAY_DATABASE_URL to be set");
            }
            return Ok(Arc::new(MemoryStore::new()));
        };

        let store = PostgresStore::new(database_url)?;
        if self.migrate_database {
            store.migrate().await?;
        }

        Ok(Arc::new(store))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use rstest::rstest;

    #[test]
    fn parses_minimal_config() {
        let config = Config::try_parse_from([
            "templar-gateway-service",
            "--near-rpc-url",
            "https://rpc.mainnet.near.org",
            "--listen-addr",
            "1.2.3.4:3333",
            "--pyth-lazer-api-key",
            "secret-token",
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
            config.oracle_sources.pyth_hermes_url.as_str(),
            "https://hermes-beta.pyth.network/"
        );
        assert_eq!(
            config.oracle_sources.redstone_node_path,
            PathBuf::from("node")
        );
        assert_eq!(config.managed_signers.len(), 1);
        assert_eq!(config.managed_signers[0].account_id.as_str(), "test.near");
        assert_eq!(config.managed_signers[0].secret_keys.len(), 2);
    }

    #[tokio::test]
    async fn migrate_requires_database_url() {
        let config = Config::try_parse_from([
            "templar-gateway-service",
            "--migrate-database",
            "--pyth-lazer-api-key",
            "secret-token",
        ])
        .expect("config should parse");

        let error = match config.build_store().await {
            Ok(_) => panic!("migration without a database URL should fail"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("--migrate-database requires GATEWAY_DATABASE_URL to be set"));
    }

    #[tokio::test]
    async fn build_store_defaults_to_memory_without_database_url() {
        let config = Config::try_parse_from([
            "templar-gateway-service",
            "--pyth-lazer-api-key",
            "secret-token",
        ])
        .expect("config should parse");

        match config.build_store().await {
            Ok(_) => {}
            Err(error) => panic!("memory-backed default store should build: {error}"),
        }
    }

    #[rstest]
    #[case::full_config(
        &["--pyth-lazer-api-key", "secret-token", "--pyth-lazer-ws-url", "wss://example.com/v1/stream"],
        Ok(())
    )]
    #[case::blank_api_key(
        &["--pyth-lazer-api-key", "  "],
        Err("API token must not be empty")
    )]
    #[case::insecure_ws_url(
        &["--pyth-lazer-api-key", "secret-token", "--pyth-lazer-ws-url", "ws://example.com/v1/stream"],
        Err("websocket URL must use wss://")
    )]
    #[case::zero_max_payload_age(
        &["--pyth-lazer-api-key", "secret-token", "--pyth-lazer-max-payload-age-ms", "0"],
        Err("max payload age must be greater than zero")
    )]
    #[case::invalid_channel(
        &["--pyth-lazer-api-key", "secret-token", "--pyth-lazer-channel", "hourly"],
        Err("unsupported Pyth Lazer channel")
    )]
    fn lazer_config_validation(#[case] extra_args: &[&str], #[case] expected: Result<(), &str>) {
        let mut args = vec!["templar-gateway-service"];
        args.extend_from_slice(extra_args);
        let config = Config::try_parse_from(args).expect("config should parse");

        match expected {
            Ok(()) => {
                config
                    .oracle_sources
                    .build()
                    .expect("valid Lazer config should build");
            }
            Err(expected_msg) => {
                let error = config
                    .oracle_sources
                    .build()
                    .expect_err("invalid Lazer config should fail");
                assert!(error.to_string().contains(expected_msg));
            }
        }
    }
}
