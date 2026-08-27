//! Raw sandbox RPC and launch configuration shared by tests and tools.

use std::time::Duration;

use anyhow::{Context, Result};
use near_api::{types::AccountId, NetworkConfig};
use near_jsonrpc_client::{
    methods::{
        query::RpcQueryRequest, sandbox_fast_forward::RpcSandboxFastForwardRequest,
        sandbox_patch_state::RpcSandboxPatchStateRequest, status::RpcStatusRequest,
    },
    JsonRpcClient,
};
use near_primitives::{
    state_record::StateRecord,
    types::{BlockReference, Finality, StoreKey, StoreValue},
    views::QueryRequest,
};
use near_sandbox::{
    config::{DEFAULT_GENESIS_ACCOUNT_PRIVATE_KEY, DEFAULT_GENESIS_ACCOUNT_PUBLIC_KEY},
    GenesisAccount, SandboxConfig,
};
use near_token::NearToken;
use serde::Serialize;

const RPC_TIMEOUT: Duration = Duration::from_secs(120);
const FINALITY_TIMEOUT: Duration = Duration::from_secs(60);
const FINALITY_POLL_MIN: Duration = Duration::from_millis(25);
const FINALITY_POLL_MAX: Duration = Duration::from_millis(500);
const STOCK_MIN_BLOCK_MS: u64 = 120;
const STOCK_MAX_BLOCK_MS: u64 = 500;
const FAST_FORWARD_BLOCK_MS: u64 = (STOCK_MIN_BLOCK_MS + STOCK_MAX_BLOCK_MS) / 2;
const MIN_BLOCK_MS: u64 = 40;

/// The high-balance genesis account used by sandbox harnesses. It reuses the
/// default genesis keypair because shared test runs exhaust `sandbox`.
pub const FUNDER_ACCOUNT_ID: &str = "funder";
fn build_client(rpc_url: &str, timeout: Duration) -> JsonRpcClient {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    let http = reqwest::Client::builder()
        .timeout(timeout)
        .default_headers(headers)
        .build()
        .unwrap_or_else(|error| panic!("reqwest client builds: {error}"));
    JsonRpcClient::with(http).connect(rpc_url)
}

fn client(network: &NetworkConfig) -> JsonRpcClient {
    build_client(network.rpc_endpoints[0].url.as_str(), RPC_TIMEOUT)
}

/// Whether the sandbox node at `rpc_url` answers a status query within `timeout`.
pub async fn node_is_serving(rpc_url: &str, timeout: Duration) -> bool {
    build_client(rpc_url, timeout)
        .call(RpcStatusRequest)
        .await
        .is_ok()
}

/// Patch raw contract storage entries on `account_id`.
pub async fn patch_data(
    network: &NetworkConfig,
    account_id: &AccountId,
    entries: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
) -> Result<()> {
    let records = entries
        .into_iter()
        .map(|(key, value)| StateRecord::Data {
            account_id: account_id.clone(),
            data_key: StoreKey::from(key),
            value: StoreValue::from(value),
        })
        .collect();
    patch_records(network, records).await
}

/// State patches are optimistic; call [`wait_until_final`] before signing with a
/// patched key, because near-api reads signer keys at `Final`.
pub async fn patch_records(network: &NetworkConfig, records: Vec<StateRecord>) -> Result<()> {
    client(network)
        .call(RpcSandboxPatchStateRequest { records })
        .await
        .context("sandbox_patch_state failed")?;
    Ok(())
}

/// Advance the sandbox chain by `delta_height` blocks.
pub async fn fast_forward(network: &NetworkConfig, delta_height: u64) -> Result<()> {
    client(network)
        .call(RpcSandboxFastForwardRequest { delta_height })
        .await
        .context("sandbox_fast_forward failed")?;
    Ok(())
}

/// Wait for a patched key to reach `Final` before using it to sign, with bounded
/// backoff to avoid amplifying an overloaded sandbox node.
pub async fn wait_until_final(
    network: &NetworkConfig,
    account_id: &AccountId,
    public_key: &near_crypto::PublicKey,
) -> Result<()> {
    let client = client(network);
    let request = RpcQueryRequest {
        block_reference: BlockReference::Finality(Finality::Final),
        request: QueryRequest::ViewAccessKey {
            account_id: account_id.clone(),
            public_key: public_key.clone(),
        },
    };
    tokio::time::timeout(FINALITY_TIMEOUT, async {
        let mut backoff = FINALITY_POLL_MIN;
        while client.call(&request).await.is_err() {
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(FINALITY_POLL_MAX);
        }
    })
    .await
    .with_context(|| {
        format!(
            "patched account {account_id} never reached final finality within \
             {FINALITY_TIMEOUT:?} — the sandbox node is likely overloaded or down"
        )
    })
}

/// Sandbox launch configuration shared by owned and out-of-band nodes.
#[must_use]
pub fn sandbox_config() -> SandboxConfig {
    let (min_block_ms, max_block_ms) = block_delays_ms();
    SandboxConfig {
        additional_config: Some(
            serde_json::to_value(AdditionalConfig {
                consensus: ConsensusConfig {
                    min_block_production_delay: duration_json(min_block_ms),
                    max_block_production_delay: duration_json(max_block_ms),
                },
            })
            .unwrap_or_else(|error| panic!("sandbox config serializes: {error}")),
        ),
        additional_accounts: vec![GenesisAccount {
            account_id: FUNDER_ACCOUNT_ID
                .parse()
                .unwrap_or_else(|error| panic!("funder account id is valid: {error}")),
            public_key: DEFAULT_GENESIS_ACCOUNT_PUBLIC_KEY.to_string(),
            private_key: DEFAULT_GENESIS_ACCOUNT_PRIVATE_KEY.to_string(),
            balance: NearToken::from_near(100_000_000),
        }],
        ..SandboxConfig::default()
    }
}

#[derive(Serialize)]
struct AdditionalConfig {
    consensus: ConsensusConfig,
}

#[derive(Serialize)]
struct ConsensusConfig {
    min_block_production_delay: DurationJson,
    max_block_production_delay: DurationJson,
}

#[derive(Serialize)]
struct DurationJson {
    secs: u64,
    nanos: u64,
}

fn duration_json(ms: u64) -> DurationJson {
    DurationJson {
        secs: ms / 1_000,
        nanos: (ms % 1_000) * 1_000_000,
    }
}

fn block_delays_ms() -> (u64, u64) {
    let min = match std::env::var("NEAR_SANDBOX_BLOCK_MS") {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .unwrap_or_else(|_| {
                panic!(
                    "NEAR_SANDBOX_BLOCK_MS must be a whole number of milliseconds, got `{value}`"
                )
            })
            .clamp(1, FAST_FORWARD_BLOCK_MS),
        Err(_) => MIN_BLOCK_MS,
    };
    (min, 2 * FAST_FORWARD_BLOCK_MS - min)
}
