use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{bail, Context, Result};
use clap::Parser;
use near_account_id::AccountId;
use near_api::types::SecretKey;
use templar_gateway_core::SharedOperationStore;
use templar_gateway_oracle_updates_dispatch::{LazerSourceConfig, LazerSubscriptionConfig};
use templar_gateway_runtime::ManagedSigner;
use templar_gateway_store::{MemoryStore, PostgresStore};
use templar_gateway_types::ManagedAccountId;
use url::Url;

const DEFAULT_PYTH_LAZER_WS_URL: &str = "wss://pyth-lazer-0.dourolabs.app/v1/stream";
const DEFAULT_PYTH_LAZER_MAX_PAYLOAD_AGE_MS: u64 = 5_000;

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
    pub near_rpc_api_key: Option<String>,

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

    /// Bearer token for optional Pyth Pro/Lazer websocket payload updates.
    #[arg(long, env = "PYTH_LAZER_API_KEY")]
    pub pyth_lazer_api_key: Option<String>,

    /// Pyth Pro/Lazer websocket endpoint for optional payload updates.
    #[arg(long, env = "PYTH_LAZER_WS_URL")]
    pub pyth_lazer_ws_url: Option<Url>,

    /// Comma-separated Pyth Pro/Lazer u32 feed ids to subscribe to.
    #[arg(long, env = "PYTH_LAZER_FEED_IDS", value_delimiter = ',')]
    pub pyth_lazer_feed_ids: Vec<u32>,

    /// Pyth Pro/Lazer websocket channel.
    #[arg(long, env = "PYTH_LAZER_CHANNEL")]
    pub pyth_lazer_channel: Option<String>,

    /// Maximum age, in milliseconds, for cached Pyth Pro/Lazer payloads.
    #[arg(long, env = "PYTH_LAZER_MAX_PAYLOAD_AGE_MS")]
    pub pyth_lazer_max_payload_age_ms: Option<u64>,

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

    pub fn build_lazer_source_config(&self) -> Result<Option<LazerSourceConfig>> {
        let has_required_config =
            self.pyth_lazer_api_key.is_some() && !self.pyth_lazer_feed_ids.is_empty();
        let has_any_lazer_config = self.pyth_lazer_api_key.is_some()
            || self.pyth_lazer_ws_url.is_some()
            || !self.pyth_lazer_feed_ids.is_empty()
            || self.pyth_lazer_channel.is_some()
            || self.pyth_lazer_max_payload_age_ms.is_some();

        if !has_any_lazer_config {
            return Ok(None);
        }
        if !has_required_config {
            bail!(
                "partial Pyth Lazer config: set both PYTH_LAZER_API_KEY and PYTH_LAZER_FEED_IDS, or unset all PYTH_LAZER_* options"
            );
        }

        let ws_url = match &self.pyth_lazer_ws_url {
            Some(url) => url.clone(),
            None => DEFAULT_PYTH_LAZER_WS_URL
                .parse()
                .context("default Pyth Lazer websocket URL is invalid")?,
        };
        let api_token = self
            .pyth_lazer_api_key
            .clone()
            .context("PYTH_LAZER_API_KEY is required when Pyth Lazer is enabled")?;
        let max_payload_age = Duration::from_millis(
            self.pyth_lazer_max_payload_age_ms
                .unwrap_or(DEFAULT_PYTH_LAZER_MAX_PAYLOAD_AGE_MS),
        );

        LazerSourceConfig::new(
            ws_url,
            api_token,
            LazerSubscriptionConfig {
                price_feed_ids: self.pyth_lazer_feed_ids.clone(),
                channel: self.pyth_lazer_channel.clone(),
                max_payload_age,
            },
        )
        .map(Some)
        .map_err(anyhow::Error::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let config = Config::try_parse_from([
            "templar-gateway-service",
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
        assert!(config.build_lazer_source_config().unwrap().is_none());
        assert_eq!(config.managed_signers.len(), 1);
        assert_eq!(config.managed_signers[0].account_id.as_str(), "test.near");
        assert_eq!(config.managed_signers[0].secret_keys.len(), 2);
    }

    #[tokio::test]
    async fn migrate_requires_database_url() {
        let config = Config::try_parse_from(["templar-gateway-service", "--migrate-database"])
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
        let config =
            Config::try_parse_from(["templar-gateway-service"]).expect("config should parse");

        match config.build_store().await {
            Ok(_) => {}
            Err(error) => panic!("memory-backed default store should build: {error}"),
        }
    }

    #[test]
    fn lazer_config_is_disabled_without_env_or_args() {
        let config =
            Config::try_parse_from(["templar-gateway-service"]).expect("config should parse");

        assert!(config
            .build_lazer_source_config()
            .expect("missing Lazer config should be valid")
            .is_none());
    }

    #[test]
    fn full_lazer_config_is_enabled() {
        let config = Config::try_parse_from([
            "templar-gateway-service",
            "--pyth-lazer-api-key",
            "secret-token",
            "--pyth-lazer-feed-ids",
            "7,8",
            "--pyth-lazer-ws-url",
            "wss://example.com/v1/stream",
        ])
        .expect("config should parse");

        assert!(config
            .build_lazer_source_config()
            .expect("full Lazer config should build")
            .is_some());
    }

    #[test]
    fn partial_lazer_config_fails_clearly() {
        let config = Config::try_parse_from([
            "templar-gateway-service",
            "--pyth-lazer-api-key",
            "secret-token",
        ])
        .expect("config should parse");

        let error = config
            .build_lazer_source_config()
            .expect_err("partial Lazer config should fail");

        assert!(error.to_string().contains("partial Pyth Lazer config"));
    }

    #[test]
    fn blank_lazer_api_key_fails_clearly() {
        let config = Config::try_parse_from([
            "templar-gateway-service",
            "--pyth-lazer-api-key",
            "  ",
            "--pyth-lazer-feed-ids",
            "7",
        ])
        .expect("config should parse");

        let error = config
            .build_lazer_source_config()
            .expect_err("blank Lazer token should fail");

        assert!(error.to_string().contains("API token must not be empty"));
    }
}
