use std::collections::HashMap;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::Parser;
use near_account_id::AccountId;
use near_api::types::SecretKey;
use near_api::NetworkConfig;
use templar_gateway_client::NetworkConfigBuilder;
use templar_gateway_core::{PooledSigner, RedactedString, SharedOperationStore};
use templar_gateway_oracle_updates_dispatch::OracleSourceArgs;
use templar_gateway_store::{MemoryStore, PostgresStore};
use templar_gateway_types::ManagedAccountId;
use url::Url;

/// The URLs a network will actually request. Any embedded `apiKey` has already
/// been moved to the header by the builder, and HTTP requests never send a
/// fragment, so two URLs differing only in those reach the same node.
fn request_targets(network: &NetworkConfig) -> Vec<Url> {
    network
        .rpc_endpoints
        .iter()
        .map(|endpoint| {
            let mut url = endpoint.url.clone();
            url.set_fragment(None);
            url
        })
        .collect()
}

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

    /// Archival NEAR RPC endpoint, queried when the primary has no record of a
    /// transaction. Must retain full history: pointing this at another primary
    /// re-creates the very ambiguity it exists to settle, since a garbage
    /// collected outcome is reported exactly like one that never existed.
    /// Without an archival endpoint, reconciliation leaves such a transaction in
    /// flight rather than terminally rejecting it.
    #[arg(long, env = "NEAR_ARCHIVAL_RPC_URL")]
    pub near_archival_rpc_url: Option<Url>,

    /// API key for the RPC endpoints, sent as an `Authorization` header. May also
    /// be supplied as an `apiKey` query parameter on `--near-rpc-url`.
    #[arg(long, env = "NEAR_RPC_API_KEY")]
    pub near_rpc_api_key: Option<RedactedString>,

    /// Postgres database URL for durable gateway operation storage.
    #[arg(long, env = "GATEWAY_DATABASE_URL")]
    pub database_url: Option<String>,

    /// Run gateway Postgres migrations during startup.
    #[arg(long, env = "GATEWAY_DATABASE_MIGRATE", default_value_t = false)]
    pub migrate_database: bool,

    /// In-process oracle payload sources (Pyth Hermes, RedStone bridge, Pyth Lazer).
    /// A mainnet deployment must set `--pyth-hermes-url`; it defaults to testnet's.
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
    pub fn build_signers(&self) -> Result<HashMap<ManagedAccountId, PooledSigner>> {
        let mut signers = HashMap::new();

        for config in &self.managed_signers {
            let account_id = ManagedAccountId(config.account_id.clone());
            let entry = PooledSigner::new(account_id.clone(), config.secret_keys.iter().cloned())
                .with_context(|| {
                format!("failed to initialize signer for {}", config.account_id)
            })?;
            signers.insert(account_id, entry);
        }

        Ok(signers)
    }

    /// Resolve the primary and archival NEAR networks.
    ///
    /// The archival endpoint is a separate config rather than a second entry on
    /// the primary's, so near_api does not fail over to it for every RPC the
    /// gateway makes.
    ///
    /// # Errors
    /// Rejects the two configurations that are wrong on their face: an API key
    /// paired with a plaintext endpoint, which would send the key in cleartext;
    /// and an archival URL equal to the primary, which cannot corroborate what
    /// the primary has garbage collected.
    pub fn build_networks(&self) -> Result<(NetworkConfig, Option<NetworkConfig>)> {
        let api_key = self
            .near_rpc_api_key
            .as_ref()
            .map(|key| key.as_ref().to_owned());
        let build = |name: &str, rpc_url: Url| {
            NetworkConfigBuilder::from_url(name, rpc_url)
                .api_key(api_key.clone())
                .build()
        };

        let network = build("gateway", self.near_rpc_url.clone());
        let archival_network = self
            .near_archival_rpc_url
            .clone()
            .map(|rpc_url| build("gateway-archival", rpc_url));

        // Validate the endpoints as they will be requested rather than the URLs
        // as written: the builder moves an embedded `apiKey` onto the header, so
        // only the built form shows what actually travels, and where.
        for (flag, network) in [
            ("--near-rpc-url", Some(&network)),
            ("--near-archival-rpc-url", archival_network.as_ref()),
        ]
        .into_iter()
        .filter_map(|(flag, network)| network.map(|network| (flag, network)))
        {
            if network.rpc_endpoints.iter().any(|endpoint| {
                endpoint.bearer_header.is_some() && endpoint.url.scheme() != "https"
            }) {
                // Deliberately without the URL: it may carry the key it is being
                // rejected for exposing.
                bail!("{flag} carries an API key and must use https://, or the key travels in cleartext");
            }
        }

        if archival_network
            .as_ref()
            .is_some_and(|archival| request_targets(archival) == request_targets(&network))
        {
            bail!("--near-archival-rpc-url must differ from --near-rpc-url: an archival endpoint exists to answer what the primary has garbage collected");
        }

        Ok((network, archival_network))
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
    use templar_gateway_client::Network;

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
            config.oracle_sources.redstone.redstone_node_path,
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
    #[case::plain_http_without_key(&["--near-rpc-url", "http://localhost:3030"], Ok(()))]
    #[case::https_with_key(
        &["--near-rpc-url", "https://rpc.example.com", "--near-rpc-api-key", "SECRET"],
        Ok(())
    )]
    #[case::plain_http_with_explicit_key(
        &["--near-rpc-url", "http://rpc.example.com", "--near-rpc-api-key", "SECRET"],
        Err("must use https://")
    )]
    #[case::plain_http_with_embedded_key(
        &["--near-rpc-url", "http://rpc.example.com/?apiKey=SECRET"],
        Err("must use https://")
    )]
    #[case::plain_http_archival_with_key(
        &[
            "--near-rpc-url", "https://rpc.example.com",
            "--near-archival-rpc-url", "http://archival.example.com",
            "--near-rpc-api-key", "SECRET",
        ],
        Err("must use https://")
    )]
    #[case::archival_differs_only_by_fragment(
        &[
            "--near-rpc-url", "https://rpc.example.com/",
            "--near-archival-rpc-url", "https://rpc.example.com/#archival",
        ],
        Err("must differ from --near-rpc-url")
    )]
    #[case::archival_differs_only_by_api_key(
        &[
            "--near-rpc-url", "https://rpc.example.com/",
            "--near-archival-rpc-url", "https://rpc.example.com/?apiKey=SECRET",
        ],
        Err("must differ from --near-rpc-url")
    )]
    #[case::archival_same_as_primary(
        &[
            "--near-rpc-url", "https://rpc.example.com/",
            "--near-archival-rpc-url", "https://rpc.example.com/",
        ],
        Err("must differ from --near-rpc-url")
    )]
    #[case::distinct_archival(
        &[
            "--near-rpc-url", "https://rpc.example.com/",
            "--near-archival-rpc-url", "https://archival.example.com/",
        ],
        Ok(())
    )]
    fn network_config_validation(#[case] extra_args: &[&str], #[case] expected: Result<(), &str>) {
        let mut args = vec!["templar-gateway-service"];
        args.extend_from_slice(extra_args);
        let config = Config::try_parse_from(args).expect("config should parse");

        match (expected, config.build_networks()) {
            (Ok(()), Ok(_)) => {}
            (Ok(()), Err(error)) => panic!("valid network config should build: {error}"),
            (Err(expected_msg), Err(error)) => {
                assert!(
                    error.to_string().contains(expected_msg),
                    "expected {expected_msg:?}, got {error}"
                );
            }
            (Err(expected_msg), Ok(_)) => {
                panic!("invalid network config should fail with {expected_msg:?}")
            }
        }
    }

    /// The rejection exists to stop a key travelling in cleartext, so the
    /// rejection must not print it either.
    #[test]
    fn insecure_endpoint_error_excludes_the_api_key() {
        let config = Config::try_parse_from([
            "templar-gateway-service",
            "--near-rpc-url",
            "http://rpc.example.com/?apiKey=SUPERSECRET",
        ])
        .expect("config should parse");

        let error = config
            .build_networks()
            .expect_err("plaintext endpoint carrying a key must be rejected")
            .to_string();

        assert!(
            !error.contains("SUPERSECRET"),
            "error leaked the key: {error}"
        );
        assert!(error.contains("--near-rpc-url"));
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
                    .build(Network::Testnet.hermes_url())
                    .expect("valid Lazer config should build");
            }
            Err(expected_msg) => {
                let error = config
                    .oracle_sources
                    .build(Network::Testnet.hermes_url())
                    .expect_err("invalid Lazer config should fail");
                assert!(error.to_string().contains(expected_msg));
            }
        }
    }
}
