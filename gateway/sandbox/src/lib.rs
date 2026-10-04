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
    let client = client(network);
    for records in batches(records)? {
        client
            .call(RpcSandboxPatchStateRequest { records })
            .await
            .context("sandbox_patch_state failed")?;
    }
    Ok(())
}

/// A whole account of contract blobs in one `sandbox_patch_state` is refused as too large.
const PATCH_REQUEST_BYTES: usize = 512 * 1024;

/// Split `records` into requests of at most [`PATCH_REQUEST_BYTES`] each, in order; a single
/// larger record travels alone.
fn batches(records: Vec<StateRecord>) -> Result<Vec<Vec<StateRecord>>> {
    let mut batches: Vec<Vec<StateRecord>> = Vec::new();
    let mut size = 0;
    for record in records {
        let record_size = serde_json::to_vec(&record)?.len();
        match batches.last_mut() {
            Some(batch) if size + record_size <= PATCH_REQUEST_BYTES => batch.push(record),
            _ => {
                batches.push(vec![record]);
                size = 0;
            }
        }
        size += record_size;
    }
    Ok(batches)
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
                trie_viewer_state_size_limit: TRIE_VIEWER_STATE_SIZE_LIMIT,
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

/// The default 50 kB refuses `view_state` for any account holding a contract blob.
const TRIE_VIEWER_STATE_SIZE_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Serialize)]
struct AdditionalConfig {
    consensus: ConsensusConfig,
    trie_viewer_state_size_limit: u64,
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

#[cfg(test)]
mod tests {
    use near_primitives::{
        state_record::StateRecord,
        types::{StoreKey, StoreValue},
    };

    use super::{batches, PATCH_REQUEST_BYTES};

    fn record(index: u8, len: usize) -> StateRecord {
        StateRecord::Data {
            account_id: "registry.near".parse().unwrap(),
            data_key: StoreKey::from(vec![index]),
            value: StoreValue::from(vec![index; len]),
        }
    }

    fn index_of(record: &StateRecord) -> u8 {
        match record {
            StateRecord::Data { data_key, .. } => data_key[0],
            _ => unreachable!(),
        }
    }

    #[test]
    fn batches_keep_order_stay_bounded_and_send_an_oversized_record_alone() {
        let records = vec![
            record(0, 100),
            record(1, PATCH_REQUEST_BYTES),
            record(2, 100_000),
            record(3, 100_000),
            record(4, 300_000),
        ];

        let batches = batches(records).unwrap();

        let order: Vec<Vec<u8>> = batches
            .iter()
            .map(|batch| batch.iter().map(index_of).collect())
            .collect();
        assert_eq!(order, vec![vec![0], vec![1], vec![2, 3], vec![4]]);
        for batch in batches.iter().filter(|batch| batch.len() > 1) {
            let size: usize = batch
                .iter()
                .map(|record| serde_json::to_vec(record).unwrap().len())
                .sum();
            assert!(size <= PATCH_REQUEST_BYTES, "{size}");
        }
    }
}
