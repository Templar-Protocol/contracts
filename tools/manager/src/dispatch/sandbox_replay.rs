//! Reconstructing a live account in a fresh sandbox, so a reviewed transaction can run against its
//! exact state before it runs on chain.

use anyhow::{Context, Result};
use borsh::BorshDeserialize as _;
use near_api::{types::AccountId, NetworkConfig, SecretKey};
use near_crypto::KeyType;
use near_primitives::{
    account::{Account as ChainAccount, AccountContract},
    state_record::StateRecord,
};
use near_token::NearToken;
use templar_gateway_methods_spec::tx;
use templar_gateway_types::{ActionInput, Base64Bytes, ManagedAccountId, OperationStatus};

use crate::dispatch::patch_state::StateSnapshot;

pub(super) async fn start_sandbox() -> Result<(near_sandbox::Sandbox, NetworkConfig)> {
    let sandbox =
        near_sandbox::Sandbox::start_sandbox_with_config(templar_sandbox::sandbox_config())
            .await
            .context("start the replay sandbox")?;
    let network = NetworkConfig::from_rpc_url("sandbox", sandbox.rpc_addr.parse()?);
    Ok((sandbox, network))
}

/// Write `state` under `account_id` with no code, swapping `signing_key` for a fresh one whose
/// secret is returned.
pub(super) async fn setup_account(
    network: &NetworkConfig,
    account_id: &AccountId,
    signing_key: &near_api::types::PublicKey,
    state: &StateSnapshot,
) -> Result<SecretKey> {
    let key_type = match signing_key.key_type() {
        near_api::types::crypto::KeyType::ED25519 => KeyType::ED25519,
        near_api::types::crypto::KeyType::SECP256K1 => KeyType::SECP256K1,
    };
    let secret_key = random_secret_key(key_type)?;
    let replacement_key: near_crypto::PublicKey = secret_key
        .public_key()
        .to_string()
        .parse()
        .context("parse the sandbox signing key")?;
    anyhow::ensure!(
        state.access_keys.iter().any(|(key, _)| key == signing_key),
        "the signing key is not one of the snapshotted account's keys"
    );
    let mut records = vec![StateRecord::Account {
        account_id: account_id.clone(),
        account: ChainAccount::new(
            NearToken::from_near(100_000_000),
            NearToken::from_yoctonear(0),
            AccountContract::None,
            state.storage_usage,
        ),
    }];
    records.extend(state.entries.iter().map(|entry| StateRecord::Data {
        account_id: account_id.clone(),
        data_key: entry.key.clone().into(),
        value: entry.value.clone().into(),
    }));
    // Keys last: batches land in order, so the replacement key reaching `Final` implies the state
    // before it has.
    for (public_key, access_key) in &state.access_keys {
        let public_key = if public_key == signing_key {
            replacement_key.clone()
        } else {
            public_key
                .to_string()
                .parse()
                .context("parse source public key")?
        };
        let mut access_key =
            near_primitives::account::AccessKey::try_from_slice(&borsh::to_vec(access_key)?)?;
        access_key.nonce = 0;
        records.push(StateRecord::AccessKey {
            account_id: account_id.clone(),
            public_key,
            access_key,
        });
    }
    templar_sandbox::patch_records(network, records).await?;
    templar_sandbox::wait_until_final(network, account_id, &replacement_key).await?;
    Ok(secret_key)
}

/// Deploy `code` as the account's own, before [`reset_account_metadata`] restores its linkage.
pub(super) async fn stage_local_code(
    client: &templar_gateway_client::Client,
    account_id: &AccountId,
    code: &[u8],
) -> Result<()> {
    let staging = client
        .execute_as(
            ManagedAccountId(account_id.clone()),
            tx::Batch {
                receiver_id: account_id.clone(),
                actions: vec![ActionInput::DeployContract {
                    code: Base64Bytes(code.to_vec()),
                }],
            },
        )
        .await?;
    anyhow::ensure!(
        staging.operation.status == OperationStatus::Succeeded,
        "staging the account's code in the sandbox failed: {:?}",
        staging.operation.status
    );
    Ok(())
}

/// Restore the snapshot's balance, code linkage and storage usage once its code is staged.
pub(super) async fn reset_account_metadata(
    network: &NetworkConfig,
    account_id: &AccountId,
    state: &StateSnapshot,
) -> Result<()> {
    templar_sandbox::patch_records(
        network,
        vec![StateRecord::Account {
            account_id: account_id.clone(),
            account: ChainAccount::new(
                state.amount,
                state.locked,
                state.contract.clone(),
                state.storage_usage,
            ),
        }],
    )
    .await
}

pub(super) fn build_local_client(
    network: &NetworkConfig,
    account_id: &AccountId,
    secret_key: &SecretKey,
) -> Result<templar_gateway_client::Client> {
    templar_gateway_client::Client::builder(network.clone())
        .secret_key(account_id.clone(), secret_key.clone())?
        .build()
        .context("build the sandbox client")
}

pub(super) fn random_secret_key(key_type: KeyType) -> Result<SecretKey> {
    near_crypto::SecretKey::from_random(key_type)
        .to_string()
        .parse()
        .context("parse the generated sandbox key")
}
