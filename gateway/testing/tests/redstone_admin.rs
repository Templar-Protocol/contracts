//! Actual-WASM ownership bootstrap, configuration, and keyless self-upgrade receipts.
use std::fmt::Write;

use anyhow::Result;
use near_api::types::transaction::result::{ExecutionSuccess, TransactionResult};
use near_api::types::AccountId;
use near_api::{Account, Contract};
use near_sdk::json_types::Base64VecU8;
use near_sdk::{Gas, NearToken};
use serde_json::{json, Value};
use templar_common::oracle::redstone::{config, Config};
use templar_common::upgrade::UpgradeSource;
use templar_gateway_testing::{
    test_signer, wasm, ArtifactId, SandboxHarness, TEST_FINALITY_POLICY,
};

const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);

fn payload() -> Result<Base64VecU8> {
    let fixture =
        include_str!("../../../contract/redstone-adapter/tests/fixtures/eth_btc.hex").trim();
    Ok(Base64VecU8(
        (0..fixture.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&fixture[offset..offset + 2], 16))
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

async fn snapshot(h: &SandboxHarness, adapter: &AccountId) -> Result<Value> {
    let mut roles = Vec::new();
    for role in ["ModifyRoles", "TrustedUpdater"] {
        let mut members: Vec<AccountId> = h
            .view_json(adapter, "list_role", json!({"role": role}))
            .await?;
        members.sort();
        roles.push(members);
    }
    Ok(json!({
        "config": h.view_json::<Value>(adapter, "get_config", json!({})).await?,
        "feeds": h.view_json::<Value>(adapter, "read_price_data", json!({"feed_ids": ["ETH", "BTC"]})).await?,
        "roles": roles,
    }))
}

async fn ownership(h: &SandboxHarness, adapter: &AccountId) -> Result<Value> {
    Ok(json!({
        "owner": h.view_json::<Value>(adapter, "own_get_owner", json!({})).await?,
        "proposed": h.view_json::<Value>(adapter, "own_get_proposed_owner", json!({})).await?,
    }))
}

async fn code_identity(h: &SandboxHarness, adapter: &AccountId) -> Result<String> {
    Ok(format!(
        "{:?}",
        Account(adapter.clone())
            .view()
            .at(TEST_FINALITY_POLICY.query_reference())
            .fetch_from(&h.network)
            .await?
            .data
            .contract_state
    ))
}

async fn call(
    h: &SandboxHarness,
    adapter: &AccountId,
    actor: &AccountId,
    method: &str,
    args: Value,
    deposit: NearToken,
    gas: Gas,
) -> Result<TransactionResult> {
    Ok(Contract(adapter.clone())
        .call_function(method, args)
        .transaction()
        .deposit(deposit)
        .gas(gas)
        .with_signer(actor.clone(), test_signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(&h.network)
        .await?)
}

fn successful(result: TransactionResult) -> ExecutionSuccess {
    let outcome = result.assert_success();
    assert!(
        outcome.receipt_failures().is_empty(),
        "asynchronous receipt failures: {:#?}",
        outcome.receipt_failures()
    );
    outcome
}

// Keep one ordered lifecycle so the same released feed records and roles are compared
// across bootstrap, configuration, and both code-linkage transitions.
#[allow(
    clippy::too_many_lines,
    reason = "one released-state lifecycle proves preservation across atomic upgrade receipts"
)]
#[tokio::test]
async fn released_state_survives_direct_administration_and_keyless_upgrades() -> Result<()> {
    let h = SandboxHarness::start().await?;
    let adapter = h.create_user("redstone-legacy").await?;
    let owner = h.create_user("redstone-owner").await?;
    let updater = h.create_user("redstone-updater").await?;
    let pending = h.create_user("redstone-pending").await?;
    let mut config = config::prod();
    config.max_timestamp_delay_ms = u64::MAX;
    config.max_timestamp_ahead_ms = u64::MAX;
    config.min_interval_between_updates_ms = 0;
    h.deploy_and_init(
        &adapter.0,
        wasm::released(ArtifactId::RedstoneAdapter, "0.1.0").await,
        "new",
        json!({"config": config}),
    )
    .await?;
    for role in ["ModifyRoles", "TrustedUpdater"] {
        h.call_function_payable(
            &adapter,
            &adapter.0,
            "set_role",
            json!({"account_id": updater.0, "role": role, "set": true}),
            ONE_YOCTO,
        )
        .await?;
    }
    h.call_function(
        &updater,
        &adapter.0,
        "write_prices",
        json!({"feed_ids": ["ETH", "BTC"], "payload": payload()?}),
    )
    .await?;
    let before = snapshot(&h, &adapter.0).await?;
    assert_eq!(before["feeds"]["ETH"]["price"], json!("195692129540"));
    assert_eq!(before["feeds"]["BTC"]["price"], json!("6698556748915"));
    for feed in ["ETH", "BTC"] {
        assert_eq!(
            before["feeds"][feed]["package_timestamp"],
            json!("1770985144000000000")
        );
        assert!(before["feeds"][feed]["write_timestamp"].as_str().is_some());
    }
    let released_code = code_identity(&h, &adapter.0).await?;
    let code = wasm::redstone_adapter().await.to_vec();
    let rejected = h
        .try_deploy_and_init(&adapter.0, code.clone(), "migrate", json!({}))
        .await?;
    assert!(rejected
        .operation
        .failure_message()
        .unwrap_or_default()
        .contains("migrate: owner_id is required for uninitialized ownership"));
    assert_eq!(code_identity(&h, &adapter.0).await?, released_code);
    assert_eq!(snapshot(&h, &adapter.0).await?, before);
    h.deploy_and_init(
        &adapter.0,
        code.clone(),
        "migrate",
        json!({"owner_id": owner.0}),
    )
    .await?;
    assert_ne!(code_identity(&h, &adapter.0).await?, released_code);
    assert_eq!(snapshot(&h, &adapter.0).await?, before);
    assert_eq!(
        ownership(&h, &adapter.0).await?,
        json!({"owner": owner.0, "proposed": null})
    );
    let rejected = h
        .try_call_function(&owner, &adapter.0, "migrate", json!({}))
        .await?;
    assert!(rejected
        .operation
        .failure_message()
        .unwrap_or_default()
        .contains("private"));
    assert_eq!(snapshot(&h, &adapter.0).await?, before);

    let mut replacement: Config = serde_json::from_value(before["config"].clone())?;
    replacement.max_timestamp_delay_ms = 120_000;
    let outcome = successful(
        call(
            &h,
            &adapter.0,
            &owner.0,
            "admin_set_config",
            json!({"config": replacement}),
            ONE_YOCTO,
            Gas::from_tgas(50),
        )
        .await?,
    );
    let events = outcome
        .logs()
        .iter()
        .filter_map(|log| log.strip_prefix("EVENT_JSON:"))
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    let events: Vec<_> = events
        .into_iter()
        .filter(|event| {
            event["standard"] == "redstone-adapter" && event["event"] == "config_updated"
        })
        .collect();
    let mut event_config = serde_json::to_value(&replacement)?;
    let mut signers = Vec::with_capacity(replacement.signers.len());
    for address in &replacement.signers {
        let mut encoded = String::with_capacity(40);
        for byte in address {
            write!(encoded, "{byte:02x}")?;
        }
        signers.push(encoded);
    }
    event_config["signers"] = json!(signers);
    assert_eq!(
        events,
        vec![json!({
            "standard": "redstone-adapter", "version": "1.0.0", "event": "config_updated",
            "data": {"updater": owner.0, "config": event_config}
        })]
    );
    h.call_function_payable(
        &owner,
        &adapter.0,
        "own_propose_owner",
        json!({"account_id": pending.0}),
        ONE_YOCTO,
    )
    .await?;
    let preserved = snapshot(&h, &adapter.0).await?;
    assert_eq!(preserved["config"], serde_json::to_value(replacement)?);
    assert_eq!(preserved["feeds"], before["feeds"]);
    assert_eq!(preserved["roles"], before["roles"]);
    let preserved_owner = ownership(&h, &adapter.0).await?;
    h.revoke_all_access_keys(&adapter.0).await?;
    assert!(!h.view_access_keys(&owner.0).await?.is_empty());
    let local_identity = code_identity(&h, &adapter.0).await?;
    assert!(local_identity.contains("LocalHash"));
    let global_hash = h.deploy_global_contract(code.clone()).await?;
    for source in [
        UpgradeSource::GlobalHash(global_hash.0.into()),
        UpgradeSource::Code(Base64VecU8(code)),
    ] {
        successful(
            call(
                &h,
                &adapter.0,
                &owner.0,
                "admin_upgrade",
                json!({
                    "code": source, "migrate_args": Base64VecU8(b"{}".to_vec())
                }),
                ONE_YOCTO,
                Gas::from_tgas(500),
            )
            .await?,
        );
        let identity = code_identity(&h, &adapter.0).await?;
        assert!(
            identity.contains(if matches!(source, UpgradeSource::GlobalHash(_)) {
                "GlobalHash"
            } else {
                "LocalHash"
            })
        );
        assert_eq!(snapshot(&h, &adapter.0).await?, preserved);
        assert_eq!(ownership(&h, &adapter.0).await?, preserved_owner);
        let rejected = call(
            &h,
            &adapter.0,
            &owner.0,
            "admin_upgrade",
            json!({
                "code": UpgradeSource::Code(Base64VecU8(wasm::redstone_adapter().await.to_vec())),
                "migrate_args": Base64VecU8(serde_json::to_vec(&json!({"owner_id": owner.0}))?)
            }),
            ONE_YOCTO,
            Gas::from_tgas(500),
        )
        .await?;
        let error = rejected
            .into_result()
            .expect_err("initialized ownership cannot be bootstrapped again");
        assert!(format!("{error:?}").contains("migrate: owner is already initialized"));
        assert_eq!(code_identity(&h, &adapter.0).await?, identity);
        assert_eq!(snapshot(&h, &adapter.0).await?, preserved);
        assert_eq!(ownership(&h, &adapter.0).await?, preserved_owner);
    }
    assert_eq!(code_identity(&h, &adapter.0).await?, local_identity);
    assert!(h.view_access_keys(&adapter.0).await?.is_empty());
    Ok(())
}
