use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine as _;
use near_api::{
    types::{account::ContractState, AccountId, CryptoHash, Reference},
    Account, Contract, NetworkConfig, RPCEndpoint,
};
use near_primitives::{account::AccountContract, hash::CryptoHash as ChainCryptoHash};
use near_token::NearToken;
use serde::{Deserialize, Serialize};
use templar_gateway_types::ProtocolLimits;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RawStateEntry {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateSnapshot {
    pub amount: NearToken,
    pub locked: NearToken,
    pub storage_usage: u64,
    pub contract: AccountContract,
    pub code: Vec<u8>,
    pub access_keys: Vec<(near_api::types::PublicKey, near_api::types::AccessKey)>,
    pub entries: Vec<RawStateEntry>,
    pub block_hash: CryptoHash,
    pub request_count: usize,
}

#[derive(Serialize)]
struct SnapshotDigest<'a> {
    amount: u128,
    locked: u128,
    storage_usage: u64,
    contract: &'a AccountContract,
    code: &'a [u8],
    access_keys: &'a [(near_api::types::PublicKey, near_api::types::AccessKey)],
    entries: &'a [RawStateEntry],
}

impl StateSnapshot {
    pub fn digest(&self) -> Result<crate::spec::plan::WireSha256Digest> {
        let snapshot = SnapshotDigest {
            amount: self.amount.as_yoctonear(),
            locked: self.locked.as_yoctonear(),
            storage_usage: self.storage_usage,
            contract: &self.contract,
            code: &self.code,
            access_keys: &self.access_keys,
            entries: &self.entries,
        };
        Ok(crate::spec::plan::digest(&serde_json::to_vec(&snapshot)?))
    }
}

pub async fn fetch_complete_state(
    network: &NetworkConfig,
    account_id: &AccountId,
    block_hash: CryptoHash,
    limits: &ProtocolLimits,
) -> Result<StateSnapshot> {
    let account = Account(account_id.clone())
        .view()
        .at(Reference::AtBlockHash(block_hash))
        .fetch_from(network)
        .await
        .with_context(|| format!("fetch account {account_id} at {block_hash}"))?;
    ensure_block(account.block_hash, block_hash, "account")?;

    let contract = account_contract(&account.data.contract_state);
    let code = fetch_code(
        network,
        account_id,
        &account.data.contract_state,
        block_hash,
    )
    .await?;
    let mut access_keys = Account(account_id.clone())
        .list_keys()
        .at(Reference::AtBlockHash(block_hash))
        .fetch_from(network)
        .await
        .with_context(|| format!("fetch access keys for {account_id} at {block_hash}"))?;
    ensure_block(access_keys.block_hash, block_hash, "access keys")?;
    access_keys
        .data
        .sort_by(|(left, _), (right, _)| left.cmp(right));

    let (entries, request_count) = fetch_entries(network, account_id, block_hash).await?;
    verify_storage_usage(
        account.data.storage_usage,
        &contract,
        &code,
        &access_keys.data,
        &entries,
        limits,
    )?;

    Ok(StateSnapshot {
        amount: account.data.amount,
        locked: account.data.locked,
        storage_usage: account.data.storage_usage,
        contract,
        code,
        access_keys: access_keys.data,
        entries,
        block_hash,
        request_count,
    })
}

fn account_contract(state: &ContractState) -> AccountContract {
    match state {
        ContractState::GlobalHash(hash) => AccountContract::Global(ChainCryptoHash(hash.0)),
        ContractState::GlobalAccountId(account_id) => {
            AccountContract::GlobalByAccount(account_id.clone())
        }
        ContractState::LocalHash(hash) => AccountContract::Local(ChainCryptoHash(hash.0)),
        ContractState::None => AccountContract::None,
    }
}

async fn fetch_code(
    network: &NetworkConfig,
    account_id: &AccountId,
    state: &ContractState,
    block_hash: CryptoHash,
) -> Result<Vec<u8>> {
    let code = match state {
        ContractState::GlobalHash(hash) => Contract::global_wasm()
            .by_hash(*hash)
            .at(Reference::AtBlockHash(block_hash))
            .fetch_from(network)
            .await
            .with_context(|| format!("fetch global code {hash} at {block_hash}"))?,
        ContractState::GlobalAccountId(publisher) => Contract::global_wasm()
            .by_account_id(publisher.clone())
            .at(Reference::AtBlockHash(block_hash))
            .fetch_from(network)
            .await
            .with_context(|| format!("fetch global code from {publisher} at {block_hash}"))?,
        ContractState::LocalHash(_) => Account(account_id.clone())
            .as_contract()
            .wasm()
            .at(Reference::AtBlockHash(block_hash))
            .fetch_from(network)
            .await
            .with_context(|| format!("fetch code for {account_id} at {block_hash}"))?,
        ContractState::None => return Ok(Vec::new()),
    };
    ensure_block(code.block_hash, block_hash, "code")?;
    base64::engine::general_purpose::STANDARD
        .decode(code.data.code_base64)
        .context("decode contract code")
}

#[derive(Serialize)]
struct StateQueryRequest<'a> {
    jsonrpc: &'static str,
    id: &'static str,
    method: &'static str,
    params: StateQueryParams<'a>,
}

#[derive(Serialize)]
struct StateQueryParams<'a> {
    request_type: &'static str,
    account_id: &'a AccountId,
    prefix_base64: &'static str,
    block_id: CryptoHash,
    limit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    after_key_base64: Option<&'a str>,
}

#[derive(Deserialize)]
struct StateQueryResponse {
    result: Option<StateQueryPage>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct StateQueryPage {
    values: Vec<StateQueryEntry>,
    last_key: Option<String>,
}

#[derive(Deserialize)]
struct StateQueryEntry {
    key: String,
    value: String,
}

fn rpc_client(endpoint: &RPCEndpoint) -> Result<reqwest::Client> {
    let timeout = Duration::from_secs(15);
    let mut builder = reqwest::Client::builder()
        .connect_timeout(timeout)
        .timeout(timeout);
    if let Some(bearer_header) = &endpoint.bearer_header {
        let mut headers = reqwest::header::HeaderMap::new();
        let mut header = bearer_header
            .parse::<reqwest::header::HeaderValue>()
            .context("invalid RPC API key header")?;
        header.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, header.clone());
        headers.insert(
            reqwest::header::HeaderName::from_static("x-api-key"),
            header,
        );
        builder = builder.default_headers(headers);
    }
    builder.build().context("build contract storage RPC client")
}

async fn fetch_state_page(
    network: &NetworkConfig,
    request: &StateQueryRequest<'_>,
) -> Result<(StateQueryPage, usize)> {
    let mut last_error = None;
    let mut request_count = 0;
    for endpoint in &network.rpc_endpoints {
        let client = rpc_client(endpoint)?;
        let attempts = usize::from(endpoint.retries).max(1);
        for attempt in 0..attempts {
            request_count += 1;
            let response = async {
                let response: StateQueryResponse = client
                    .post(endpoint.url.clone())
                    .json(request)
                    .send()
                    .await
                    .context("request contract storage")?
                    .error_for_status()
                    .context("contract storage RPC returned an HTTP error")?
                    .json()
                    .await
                    .context("decode contract storage RPC response")?;
                if let Some(error) = response.error {
                    bail!("contract storage RPC failed: {error}");
                }
                response
                    .result
                    .context("contract storage RPC omitted result")
            }
            .await;
            match response {
                Ok(page) => return Ok((page, request_count)),
                Err(error) => {
                    last_error = Some(error);
                    if attempt + 1 < attempts {
                        tokio::time::sleep(endpoint.get_sleep_duration(attempt)).await;
                    }
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("network has no RPC endpoint")))
}

#[allow(
    clippy::too_many_lines,
    reason = "the cursor RPC wire types and their single pagination loop share one request boundary"
)]
async fn fetch_entries(
    network: &NetworkConfig,
    account_id: &AccountId,
    block_hash: CryptoHash,
) -> Result<(Vec<RawStateEntry>, usize)> {
    let mut entries = Vec::new();
    let mut after_key = None;
    let mut request_count = 0;
    loop {
        let (page, page_request_count) = fetch_state_page(
            network,
            &StateQueryRequest {
                jsonrpc: "2.0",
                id: "patch-state",
                method: "query",
                params: StateQueryParams {
                    request_type: "view_state",
                    account_id,
                    prefix_base64: "",
                    block_id: block_hash,
                    limit: 100,
                    after_key_base64: after_key.as_deref(),
                },
            },
        )
        .await?;
        request_count += page_request_count;
        entries.extend(
            page.values
                .into_iter()
                .map(|entry| {
                    Ok(RawStateEntry {
                        key: base64::engine::general_purpose::STANDARD.decode(entry.key)?,
                        value: base64::engine::general_purpose::STANDARD.decode(entry.value)?,
                    })
                })
                .collect::<std::result::Result<Vec<_>, base64::DecodeError>>()?,
        );
        let Some(next_key) = page.last_key else {
            break;
        };
        ensure!(
            after_key.as_deref() != Some(next_key.as_str()),
            "contract storage cursor did not advance"
        );
        after_key = Some(next_key);
    }
    entries.sort_unstable_by(|left, right| left.key.cmp(&right.key));
    for pair in entries.windows(2) {
        ensure!(
            pair[0].key != pair[1].key,
            "duplicate storage key in state response"
        );
    }
    Ok((entries, request_count))
}

fn ensure_block(actual: CryptoHash, expected: CryptoHash, source: &str) -> Result<()> {
    ensure!(
        actual == expected,
        "{source} response is pinned to {actual}, expected {expected}"
    );
    Ok(())
}

fn verify_storage_usage(
    expected: u64,
    contract: &AccountContract,
    code: &[u8],
    keys: &[(near_api::types::PublicKey, near_api::types::AccessKey)],
    entries: &[RawStateEntry],
    limits: &ProtocolLimits,
) -> Result<()> {
    let mut accounted = u128::from(limits.num_bytes_account)
        + match contract {
            AccountContract::Local(_) => u128::try_from(code.len())?,
            _ => u128::from(contract.identifier_storage_usage()),
        };
    for (public_key, access_key) in keys {
        accounted += u128::try_from(borsh::object_length(public_key)?)?;
        accounted += u128::try_from(borsh::object_length(access_key)?)?;
        accounted += u128::from(limits.num_extra_bytes_record);
    }
    for entry in entries {
        accounted += u128::try_from(entry.key.len())?;
        accounted += u128::try_from(entry.value.len())?;
        accounted += u128::from(limits.num_extra_bytes_record);
    }
    let accounted = u64::try_from(accounted).context("accounted storage usage exceeds u64")?;
    ensure!(
        accounted == expected,
        "complete state accounting mismatch: account reports {expected} bytes, fetched records account for {accounted}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> ProtocolLimits {
        ProtocolLimits {
            max_transaction_size: 1,
            max_total_prepaid_gas: templar_gateway_types::NearGas::from_gas(1),
            max_length_storage_key: 16,
            max_length_storage_value: 16,
            num_bytes_account: 100,
            num_extra_bytes_record: 40,
        }
    }

    #[test]
    fn complete_state_accounting_rejects_omitted_entries() {
        let entries = vec![RawStateEntry {
            key: vec![1],
            value: vec![2, 3],
        }];
        let expected = 100 + 3 + 1 + 2 + 40;
        verify_storage_usage(
            expected,
            &AccountContract::Local(ChainCryptoHash([0; 32])),
            b"abc",
            &[],
            &entries,
            &limits(),
        )
        .expect("complete state is accounted");
        assert!(verify_storage_usage(
            expected,
            &AccountContract::Local(ChainCryptoHash([0; 32])),
            b"abc",
            &[],
            &[],
            &limits(),
        )
        .is_err());
    }

    #[test]
    fn snapshot_digest_excludes_fetch_metadata_and_binds_state() {
        let snapshot = StateSnapshot {
            amount: NearToken::from_yoctonear(1),
            locked: NearToken::from_yoctonear(2),
            storage_usage: 3,
            contract: AccountContract::Local(ChainCryptoHash([4; 32])),
            code: vec![5],
            access_keys: Vec::new(),
            entries: vec![RawStateEntry {
                key: vec![6],
                value: vec![7],
            }],
            block_hash: CryptoHash([8; 32]),
            request_count: 1,
        };
        let digest = snapshot.digest().unwrap();
        let mut refetched = snapshot.clone();
        refetched.block_hash = CryptoHash([9; 32]);
        refetched.request_count = 2;
        assert_eq!(digest, refetched.digest().unwrap());
        refetched.entries[0].value.push(10);
        assert_ne!(digest, refetched.digest().unwrap());
    }

    #[tokio::test]
    async fn state_cursor_retries_fails_over_and_preserves_endpoint_headers() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let body = r#"{"result":{"values":[{"key":"AQ==","value":"Ag=="}],"last_key":null}}"#;
        let responses = [
            "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_owned(),
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_owned(),
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        ];
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = vec![0; 4096];
                let count = stream.read(&mut bytes).await.unwrap();
                requests.push(String::from_utf8(bytes[..count].to_vec()).unwrap());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        let mut network =
            NetworkConfig::from_rpc_url("test", format!("http://{address}").parse().unwrap());
        let url = network.rpc_endpoints[0].url.clone();
        network.rpc_endpoints = vec![
            RPCEndpoint::new(url.clone()).with_retries(2),
            RPCEndpoint::new(url)
                .with_api_key("secret".to_owned())
                .with_retries(1),
        ];

        let account_id = "account.near".parse().unwrap();
        let (page, request_count) = fetch_state_page(
            &network,
            &StateQueryRequest {
                jsonrpc: "2.0",
                id: "test",
                method: "query",
                params: StateQueryParams {
                    request_type: "view_state",
                    account_id: &account_id,
                    prefix_base64: "",
                    block_id: CryptoHash([0; 32]),
                    limit: 100,
                    after_key_base64: None,
                },
            },
        )
        .await
        .unwrap();

        assert_eq!(request_count, 3);
        assert_eq!(page.values.len(), 1);
        let requests = server.await.unwrap();
        assert!(
            !requests[0].to_ascii_lowercase().contains("authorization:"),
            "first endpoint unexpectedly sent authorization"
        );
        assert!(
            requests[2]
                .to_ascii_lowercase()
                .contains("authorization: bearer secret"),
            "failover endpoint omitted authorization"
        );
        assert!(
            requests[2]
                .to_ascii_lowercase()
                .contains("x-api-key: bearer secret"),
            "failover endpoint omitted API key"
        );
    }
}
