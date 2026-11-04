use anyhow::Result;
use near_api::Contract;
use near_sdk::NearToken;
use serde_json::{json, Value};
use templar_common::oracle::redstone::config;
use templar_gateway_testing::{wasm, ArtifactId, SandboxHarness, TEST_FINALITY_POLICY};

use super::{ownership, payload, snapshot};
use crate::common::{call, code_hash, deploy_with_init, signer, view, ONE_YOCTO};

/// Exercise the immutable released binary, not a reconstruction of its historical source.
#[allow(
    clippy::too_many_lines,
    reason = "one ordered migration scenario compares the same released state across both receipts"
)]
#[tokio::test]
async fn released_adapter_bootstraps_owner_without_rewriting_state() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let adapter = harness.create_user("legacy-redstone").await?;
    let role_admin = harness.create_user("legacy-role-admin").await?;
    let updater = harness.create_user("legacy-updater").await?;
    let owner = harness.create_user("legacy-owner").await?;
    let mut config = config::prod();
    config.max_timestamp_delay_ms = u64::MAX;
    config.max_timestamp_ahead_ms = u64::MAX;
    config.min_interval_between_updates_ms = 0;

    // The historical initializer accepts only config and grants roles to its predecessor.
    deploy_with_init(
        &harness.network,
        &adapter.0,
        wasm::released(ArtifactId::RedstoneAdapter, "0.1.0").await,
        "new",
        json!({"config": config}),
    )
    .await?;
    for (account, role) in [
        (&role_admin.0, "ModifyRoles"),
        (&updater.0, "TrustedUpdater"),
    ] {
        call(
            &harness.network,
            &adapter.0,
            "set_role",
            json!({"account_id": account, "role": role, "set": true}),
            &adapter.0,
            ONE_YOCTO,
        )
        .await?;
    }
    call(
        &harness.network,
        &adapter.0,
        "write_prices",
        json!({"feed_ids": ["ETH", "BTC"], "payload": payload()}),
        &updater.0,
        NearToken::from_yoctonear(0),
    )
    .await?;
    let feeds: Value = view(
        &harness.network,
        &adapter.0,
        "read_price_data",
        json!({"feed_ids": ["ETH", "BTC"]}),
    )
    .await?;
    assert_eq!(feeds["ETH"]["price"], json!("195692129540"));
    assert_eq!(feeds["BTC"]["price"], json!("6698556748915"));
    for feed in ["ETH", "BTC"] {
        assert_eq!(
            feeds[feed]["package_timestamp"],
            json!("1770985144000000000")
        );
        assert!(feeds[feed]["write_timestamp"].as_str().is_some());
    }
    let before = snapshot(&harness, &adapter.0).await?;
    let released_code = code_hash(&harness.network, &adapter.0).await?;
    let current_wasm = wasm::redstone_adapter().await.to_vec();

    // Both actions share one receipt: failed initialization must roll the deployment back too.
    let failure = Contract::deploy(adapter.0.clone())
        .use_code(current_wasm.clone())
        .with_init_call("migrate", json!({}))?
        .max_gas()
        .with_signer(signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(&harness.network)
        .await?
        .into_result()
        .expect_err("legacy ownership bootstrap requires an explicit owner");
    let failure = format!("{failure:?}");
    assert!(
        failure.contains("migrate: owner_id is required for uninitialized ownership"),
        "unexpected migration failure: {failure}"
    );
    assert_eq!(
        code_hash(&harness.network, &adapter.0).await?,
        released_code
    );
    assert_eq!(snapshot(&harness, &adapter.0).await?, before);
    println!(
        "RedStone released 0.1.0 failed bootstrap: code={released_code}; state preserved={before}"
    );

    deploy_with_init(
        &harness.network,
        &adapter.0,
        current_wasm,
        "migrate",
        json!({"owner_id": owner.0}),
    )
    .await?;
    let migrated_code = code_hash(&harness.network, &adapter.0).await?;
    assert_ne!(migrated_code, released_code);
    assert_eq!(snapshot(&harness, &adapter.0).await?, before);
    let owner_state = ownership(&harness, &adapter.0).await?;
    assert_eq!(owner_state, json!({"owner": owner.0, "proposed": null}));
    for role in ["ModifyRoles", "TrustedUpdater"] {
        let has_role: bool = view(
            &harness.network,
            &adapter.0,
            "has_role",
            json!({"account_id": owner.0, "role": role}),
        )
        .await?;
        assert!(!has_role, "bootstrap must not grant owner the {role} role");
    }

    // Even the owner cannot invoke the private migration directly.
    let rejected = Contract(adapter.0.clone())
        .call_function("migrate", json!({}))
        .transaction()
        .gas(near_sdk::Gas::from_tgas(50))
        .with_signer(owner.0.clone(), signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(&harness.network)
        .await?
        .into_result()
        .expect_err("migration must reject a non-self predecessor");
    let rejected = format!("{rejected:?}");
    assert!(
        rejected.contains("private"),
        "unexpected rejection: {rejected}"
    );
    assert_eq!(
        code_hash(&harness.network, &adapter.0).await?,
        migrated_code
    );
    assert_eq!(snapshot(&harness, &adapter.0).await?, before);
    assert_eq!(ownership(&harness, &adapter.0).await?, owner_state);
    println!("RedStone released migration: {released_code} -> {migrated_code}; ownership={owner_state}; config, feed prices/timestamps and both role lists preserved={before}");
    Ok(())
}
