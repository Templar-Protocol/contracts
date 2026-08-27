//! Sandbox account-minting policy built on the reusable raw sandbox primitives.

use anyhow::{Context, Result};
use near_api::{types::AccountId, NetworkConfig, SecretKey};
use near_primitives::{
    account::{AccessKey, Account as ChainAccount, AccountContract},
    state_record::StateRecord,
};
use near_token::NearToken;
pub(crate) use templar_sandbox::{fast_forward, patch_data, patch_records, wait_until_final};

const ACCOUNT_STORAGE_USAGE: u64 = 182;

/// Mint each account with its balance and a full-access key over `secret_key` by
/// writing state records directly — no blocks spent producing them, and no
/// funder is debited.
///
/// One call, however many accounts: a `sandbox_patch_state` call costs ~200ms
/// regardless of how many records it carries, so minting N accounts one call at
/// a time costs N times as much as minting them together.
pub(crate) async fn create_accounts(
    network: &NetworkConfig,
    accounts: &[(AccountId, NearToken)],
    secret_key: &SecretKey,
) -> Result<()> {
    // near-api and `near_primitives` use different `PublicKey` types; cross the
    // boundary by the canonical `ed25519:…` string.
    let public_key: near_crypto::PublicKey = secret_key
        .public_key()
        .to_string()
        .parse()
        .context("parse public key")?;
    // Nonce 0 is safe: the block-height nonce floor applies only to keys added by a
    // transaction, not to patched state.
    let records = accounts
        .iter()
        .flat_map(|(account_id, balance)| {
            [
                StateRecord::Account {
                    account_id: account_id.clone(),
                    account: ChainAccount::new(
                        *balance,
                        NearToken::from_yoctonear(0),
                        AccountContract::None,
                        ACCOUNT_STORAGE_USAGE,
                    ),
                },
                StateRecord::AccessKey {
                    account_id: account_id.clone(),
                    public_key: public_key.clone(),
                    access_key: AccessKey::full_access(),
                },
            ]
        })
        .collect();
    patch_records(network, records).await?;
    // One patch applies as a unit in one block, so any one account reaching
    // final finality means every account in the batch has.
    match accounts.last() {
        Some((account_id, _)) => wait_until_final(network, account_id, &public_key).await,
        None => Ok(()),
    }
}
