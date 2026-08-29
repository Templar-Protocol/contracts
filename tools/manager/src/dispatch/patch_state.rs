use std::fmt::Debug;

use anyhow::{bail, ensure, Context, Result};
use base64::Engine as _;
use near_api::{
    types::{account::ContractState, AccountId, CryptoHash, Reference},
    Account, Contract, NetworkConfig,
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
    pub chunked: bool,
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

    let (entries, chunked, request_count) = fetch_entries(
        network,
        account_id,
        block_hash,
        limits.max_length_storage_key,
    )
    .await?;
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
        chunked,
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

#[allow(
    clippy::too_many_lines,
    reason = "the cursor RPC wire types and their single pagination loop share one request boundary"
)]
async fn fetch_entries(
    network: &NetworkConfig,
    account_id: &AccountId,
    block_hash: CryptoHash,
    _: u64,
) -> Result<(Vec<RawStateEntry>, bool, usize)> {
    #[derive(Serialize)]
    struct Request<'a> {
        jsonrpc: &'static str,
        id: &'static str,
        method: &'static str,
        params: Params<'a>,
    }
    #[derive(Serialize)]
    struct Params<'a> {
        request_type: &'static str,
        account_id: &'a AccountId,
        prefix_base64: &'static str,
        block_id: CryptoHash,
        limit: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        after_key_base64: Option<&'a str>,
    }
    #[derive(Deserialize)]
    struct Response {
        result: Option<Page>,
        error: Option<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct Page {
        values: Vec<Entry>,
        last_key: Option<String>,
    }
    #[derive(Deserialize)]
    struct Entry {
        key: String,
        value: String,
    }

    let endpoint = network
        .rpc_endpoints
        .first()
        .context("network has no RPC endpoint")?;
    let client = reqwest::Client::new();
    let mut entries = Vec::new();
    let mut after_key = None;
    let mut request_count = 0;
    loop {
        let response: Response = client
            .post(endpoint.url.clone())
            .json(&Request {
                jsonrpc: "2.0",
                id: "patch-state",
                method: "query",
                params: Params {
                    request_type: "view_state",
                    account_id,
                    prefix_base64: "",
                    block_id: block_hash,
                    limit: 100,
                    after_key_base64: after_key.as_deref(),
                },
            })
            .send()
            .await
            .context("request contract storage")?
            .error_for_status()
            .context("contract storage RPC returned an HTTP error")?
            .json()
            .await
            .context("decode contract storage RPC response")?;
        request_count += 1;
        if let Some(error) = response.error {
            bail!("contract storage RPC failed: {error}");
        }
        let page = response
            .result
            .context("contract storage RPC omitted result")?;
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
    Ok((entries, false, request_count))
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
            chunked: false,
            request_count: 1,
        };
        let digest = snapshot.digest().unwrap();
        let mut refetched = snapshot.clone();
        refetched.block_hash = CryptoHash([9; 32]);
        refetched.chunked = true;
        refetched.request_count = 2;
        assert_eq!(digest, refetched.digest().unwrap());
        refetched.entries[0].value.push(10);
        assert_ne!(digest, refetched.digest().unwrap());
    }
}
