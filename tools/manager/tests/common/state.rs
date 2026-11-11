use std::collections::BTreeMap;

use anyhow::Result;
use near_account_id::AccountId;
use templar_gateway_core::{FinalityPolicy, NearClient};
use templar_gateway_testing::SandboxHarness;

/// Independent finalized state, not a script-reported checkpoint.
pub async fn raw_storage(
    harness: &SandboxHarness,
    account: &AccountId,
) -> Result<BTreeMap<Vec<u8>, Vec<u8>>> {
    let client = NearClient::with_finality_policy(harness.network.clone(), FinalityPolicy::Final);
    Ok(client
        .contract(account.clone())
        .state_with_prefix(Vec::new())
        .await?
        .into_iter()
        .map(|(key, value)| (key.0, value.0))
        .collect())
}
