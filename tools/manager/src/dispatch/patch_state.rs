use std::{fmt::Debug, future::Future};

use anyhow::{ensure, Context, Result};
use base64::Engine as _;
use futures::{stream, StreamExt};
use near_api::{
    types::{account::ContractState, AccountId, CryptoHash, Reference},
    Account, Contract, NetworkConfig,
};
use near_primitives::{account::AccountContract, hash::CryptoHash as ChainCryptoHash};
use near_token::NearToken;
use serde::Serialize;
use templar_gateway_types::ProtocolLimits;

const STATE_READ_CONCURRENCY: usize = 8;

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

async fn fetch_entries(
    network: &NetworkConfig,
    account_id: &AccountId,
    block_hash: CryptoHash,
    max_key_length: u64,
) -> Result<(Vec<RawStateEntry>, bool, usize)> {
    let reader = |prefix: Vec<u8>| async move {
        let state = Contract(account_id.clone())
            .view_storage_with_prefix(&prefix)
            .at(Reference::AtBlockHash(block_hash))
            .fetch_from(network)
            .await
            .map_err(|error| format!("{error:?}"))?;
        ensure_block(state.block_hash, block_hash, "state").map_err(|error| error.to_string())?;
        state
            .data
            .values
            .into_iter()
            .map(|entry| {
                Ok(RawStateEntry {
                    key: base64::engine::general_purpose::STANDARD.decode(entry.key.0)?,
                    value: base64::engine::general_purpose::STANDARD.decode(entry.value.0)?,
                })
            })
            .collect::<std::result::Result<Vec<_>, base64::DecodeError>>()
            .map_err(|error| error.to_string())
    };
    collect_prefixes(max_key_length, reader).await
}

async fn collect_prefixes<F, Fut>(
    max_key_length: u64,
    fetch: F,
) -> Result<(Vec<RawStateEntry>, bool, usize)>
where
    F: Fn(Vec<u8>) -> Fut,
    Fut: Future<Output = Result<Vec<RawStateEntry>, String>>,
{
    let mut pending = vec![Vec::new()];
    let mut entries = Vec::new();
    let mut chunked = false;
    let mut request_count = 0;

    while !pending.is_empty() {
        let mut responses = stream::iter(std::mem::take(&mut pending).into_iter().map(
            |prefix| async {
                let result = fetch(prefix.clone()).await;
                (prefix, result)
            },
        ))
        .buffer_unordered(STATE_READ_CONCURRENCY);

        while let Some((prefix, result)) = responses.next().await {
            request_count += 1;
            match result {
                Ok(values) => entries.extend(values),
                Err(error) if error.contains("TooLargeContractState") => {
                    chunked = true;
                    ensure!(
                        (prefix.len() as u64) < max_key_length,
                        "storage prefix reached max key length {max_key_length}"
                    );
                    for byte in u8::MIN..=u8::MAX {
                        let mut child = prefix.clone();
                        child.push(byte);
                        pending.push(child);
                    }
                }
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "fetch storage prefix {}: {error}",
                        base64::engine::general_purpose::STANDARD.encode(prefix)
                    ));
                }
            }
        }
    }

    entries.sort_unstable_by(|left, right| left.key.cmp(&right.key));
    for pair in entries.windows(2) {
        ensure!(
            pair[0].key != pair[1].key,
            "duplicate storage key in state response"
        );
    }
    Ok((entries, chunked, request_count))
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

#[tokio::test]
async fn state_chunker_recovers_oversized_prefixes_without_partial_results() {
    let reader = |prefix: Vec<u8>| async move {
        if prefix.is_empty() || prefix == [0] {
            Err("TooLargeContractState".to_owned())
        } else if prefix == [1] {
            Ok(vec![RawStateEntry {
                key: vec![1],
                value: vec![2],
            }])
        } else {
            Ok(Vec::new())
        }
    };
    let (entries, chunked, requests) = collect_prefixes(2, reader).await.unwrap();
    assert!(chunked);
    assert_eq!(
        entries,
        vec![RawStateEntry {
            key: vec![1],
            value: vec![2],
        }]
    );
    assert_eq!(requests, 513);
}
