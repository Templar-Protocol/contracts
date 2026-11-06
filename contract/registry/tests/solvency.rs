//! NEP-145 credits must remain withdrawable across changes in contract-code storage mode.

use anyhow::{Context, Result};
use near_api::types::{account::ContractState, AccountId};
use near_sdk::json_types::Base64VecU8;
use near_token::NearToken;
use rstest::rstest;
use serde_json::{json, Value};
use templar_common::{registry::VersionSource, upgrade::UpgradeSource};
use templar_gateway_core::client::registry::GetVersionArgs;
use templar_gateway_methods_spec::tx;
use templar_gateway_testing::{failed_receipts, harness, publish_deposit_for, SandboxHarness};
use templar_gateway_types::{common::ContractArgs, ContractMethodName, ManagedAccountId, NearGas};

const ONE: NearToken = NearToken::from_yoctonear(1);
// NEAR's protocol storage stake is 10^19 yoctoNEAR per byte.
const STORAGE_BYTE_COST: u128 = 10_000_000_000_000_000_000;

fn upgrade_operation(registry: &AccountId, code: &UpgradeSource) -> tx::FunctionCall {
    tx::FunctionCall {
        receiver_id: registry.clone(),
        method_name: ContractMethodName("upgrade".to_owned()),
        // Serialize the real type: Code is a bare base64 string, not {"Code": ...}.
        args: ContractArgs::Json(json!({
            "code": code,
            "migrate_args": Base64VecU8(Vec::new()),
        })),
        gas: NearGas::from_tgas(300),
        deposit: ONE,
    }
}

async fn storage_balance(
    h: &SandboxHarness,
    registry: &AccountId,
    payer: &ManagedAccountId,
) -> Result<Value> {
    h.view_json(
        registry,
        "storage_balance_of",
        json!({"account_id": payer.0}),
    )
    .await
}

#[rstest]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one runtime scenario proves global-to-local rollback and subsequent credit withdrawal"
)]
async fn local_code_upgrade_cannot_stake_unused_storage_credits(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let h = &harness;
    let registry = h.deploy_registry().await?;
    let owner = &h.registry_signer_account_id;
    assert_eq!(registry, owner.0);
    let current = templar_gateway_testing::wasm::registry().await.to_vec();
    h.registry_add_version(
        owner,
        &registry,
        "registry-solvency",
        VersionSource::PublishGlobal(current.clone().into()),
        publish_deposit_for(current.len()),
    )
    .await?;
    let hash = h
        .gateway_client()
        .registry(registry.clone())
        .get_version_code_hash(GetVersionArgs {
            version_key: "registry-solvency".to_owned(),
        })
        .await?
        .context("published registry hash")?;
    h.execute(
        owner,
        upgrade_operation(&registry, &UpgradeSource::GlobalHash(hash)),
    )
    .await?;
    let global_mode = ContractState::GlobalHash(near_api::CryptoHash(hash.into()));
    let account = h.gateway_client().account().get(registry.clone()).await?;
    assert_eq!(account.contract_state, global_mode);

    let payer = h.create_user("solvency-payer").await?;
    let sink = h.create_user("solvency-sink").await?;
    let bounds: Value = h
        .view_json(&registry, "storage_balance_bounds", json!({}))
        .await?;
    let minimum: u128 = bounds["min"]
        .as_str()
        .context("NEP-145 decimal registration minimum")?
        .parse()?;
    assert!(minimum > 0);
    // Leave ample protocol-owned funds for the existing metadata, Owner's future slots and gas,
    // but not enough to stake a local copy of this exact Wasm. No sandbox state patch is used.
    let protocol_budget = minimum * 100;
    let local_code_stake = u128::try_from(current.len())? * STORAGE_BYTE_COST;
    assert!(protocol_budget < local_code_stake);
    assert!(u128::from(account.storage_usage) * STORAGE_BYTE_COST < protocol_budget);
    let outgoing = account
        .amount
        .as_yoctonear()
        .checked_sub(protocol_budget)
        .context("registry starts with more than the retained protocol budget")?;
    h.execute(
        owner,
        tx::Transfer {
            receiver_id: sink.0,
            amount: NearToken::from_yoctonear(outgoing),
        },
    )
    .await?;

    let funding = NearToken::from_near(10);
    assert!(funding.as_yoctonear() > local_code_stake);
    h.call_function_payable(&payer, &registry, "storage_deposit", json!({}), funding)
        .await?;
    let before = storage_balance(h, &registry, &payer).await?;
    let available = funding.as_yoctonear() - minimum;
    assert_eq!(
        before,
        json!({"total": funding.as_yoctonear().to_string(), "available": available.to_string()}),
    );
    let funded_account = h.gateway_client().account().get(registry.clone()).await?;

    let failed = h
        .try_execute(
            owner,
            upgrade_operation(&registry, &UpgradeSource::Code(current.into())),
        )
        .await?;
    assert!(failed_receipts(&failed).next().is_some());
    let reason = failed
        .operation
        .failure_message()
        .context("failed upgrade exposes receipt failure evidence")?;
    assert!(
        reason.contains("Insufficient registry balance to cover storage liabilities"),
        "expected the post-deploy storage guard, got: {reason}",
    );
    // A matching hash alone cannot distinguish local code from a global reference. The failed
    // deploy/migrate/check receipt must roll back the mode, as well as all credited balances.
    let after = h.gateway_client().account().get(registry.clone()).await?;
    assert_eq!(after.contract_state, global_mode);
    assert_eq!(after.storage_usage, funded_account.storage_usage);
    assert_eq!(storage_balance(h, &registry, &payer).await?, before);
    let version = h.contract_state_version(&registry).await?;
    assert_eq!(version.stored, 2);
    assert_eq!(version.target, 2);
    assert!(!version.needs_migration);

    let payer_before = h.gateway_client().account().get(payer.0.clone()).await?;
    h.call_function_payable(
        &payer,
        &registry,
        "storage_withdraw",
        json!({"amount": available.to_string()}),
        ONE,
    )
    .await?;
    assert_eq!(
        storage_balance(h, &registry, &payer).await?,
        json!({"total": minimum.to_string(), "available": "0"}),
    );
    // Include receipt execution, not merely a view reporting that the liability was cleared.
    // Gas is paid by the external payer, so its increase is slightly less than the withdrawal.
    let payer_after = h.gateway_client().account().get(payer.0).await?;
    assert!(
        payer_after.amount.as_yoctonear() > payer_before.amount.as_yoctonear() + available / 2,
        "the available credit must actually reach its payer",
    );
    Ok(())
}
