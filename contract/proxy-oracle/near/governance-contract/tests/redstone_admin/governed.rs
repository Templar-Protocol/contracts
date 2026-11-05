use anyhow::Result;
use near_api::types::transaction::result::{
    ExecutionSuccess, TransactionResult, TransactionResultError,
};
use near_api::types::AccountId;
use near_api::Contract;
use near_sdk::json_types::{Base64VecU8, U128};
use near_sdk::{serde, Gas, NearToken};
use rstest::rstest;
use serde_json::{json, Value};
use templar_common::{upgrade::UpgradeSource, Nanoseconds};
use templar_gateway_testing::{harness, wasm, SandboxHarness, TEST_FINALITY_POLICY};
use templar_proxy_oracle_near_governance_common::{FunctionCall, Operation};

use super::common::{
    call, call_borsh, code_hash, deploy_global_contract, revoke_all_access_keys, signer, view,
    CreateProposalArgs, ProposalIdArgs, ONE_YOCTO,
};
use super::{ownership, payload, snapshot};

const ORDINARY_GAS: Gas = Gas::from_tgas(50);

#[allow(
    clippy::expect_used,
    reason = "malformed emitted JSON must fail the sandbox proof"
)]
fn config_events(logs: &[&str]) -> Vec<Value> {
    logs.iter()
        .filter_map(|log| log.strip_prefix("EVENT_JSON:"))
        .map(|event| serde_json::from_str::<Value>(event).expect("complete event JSON"))
        .filter(|event| {
            event["standard"] == "redstone-adapter" && event["event"] == "config_updated"
        })
        .collect()
}

#[allow(
    clippy::expect_used,
    reason = "event policy must losslessly match the viewed configuration"
)]
fn assert_config_event(outcome: &ExecutionSuccess, updater: &AccountId, config: &Value) {
    let mut encoded = config.clone();
    encoded["signers"] = Value::Array(
        config["signers"]
            .as_array()
            .expect("config signer array")
            .iter()
            .map(|address| {
                let bytes: Vec<u8> = address
                    .as_array()
                    .expect("signer byte array")
                    .iter()
                    .map(|byte| {
                        u8::try_from(byte.as_u64().expect("signer byte"))
                            .expect("signer byte fits u8")
                    })
                    .collect();
                let hex = hex::encode(bytes);
                assert_eq!(hex.len(), 40);
                Value::String(hex)
            })
            .collect(),
    );
    assert_eq!(
        config_events(&outcome.logs()),
        vec![json!({
            "standard": "redstone-adapter",
            "version": "1.0.0",
            "event": "config_updated",
            "data": {"updater": updater, "config": encoded},
        })],
    );
}

fn target(method: &str, args: impl serde::Serialize, gas: Gas) -> Result<Operation> {
    Ok(Operation::TargetFunctionCall(FunctionCall {
        method_name: method.to_owned(),
        args: Base64VecU8(serde_json::to_vec(&args)?),
        attached_deposit: U128(1),
        gas,
    }))
}

async fn send(
    harness: &SandboxHarness,
    contract: &AccountId,
    actor: &AccountId,
    method: &str,
    args: impl serde::Serialize,
    deposit: NearToken,
    gas: Gas,
) -> Result<TransactionResult> {
    Ok(Contract(contract.clone())
        .call_function(method, args)
        .transaction()
        .deposit(deposit)
        .gas(gas)
        .with_signer(actor.clone(), signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(&harness.network)
        .await?)
}

fn successful(result: TransactionResult) -> ExecutionSuccess {
    let outcome = result.assert_success();
    assert!(
        outcome.receipt_failures().is_empty(),
        "detached target receipts failed: {:#?}",
        outcome.receipt_failures(),
    );
    outcome
}

async fn propose(
    harness: &SandboxHarness,
    governance: &AccountId,
    admin: &AccountId,
    id: u32,
    operation: Operation,
    borsh: bool,
) -> Result<()> {
    let args = CreateProposalArgs {
        id,
        operation,
        requested_ttl: Nanoseconds::zero(),
    };
    if borsh {
        call_borsh(
            &harness.network,
            governance,
            "create_proposal_borsh",
            args,
            admin,
            ONE_YOCTO,
        )
        .await?;
    } else {
        call(
            &harness.network,
            governance,
            "create_proposal",
            args,
            admin,
            ONE_YOCTO,
        )
        .await?;
    }
    Ok(())
}

async fn execute(
    harness: &SandboxHarness,
    governance: &AccountId,
    admin: &AccountId,
    id: u32,
    gas: Gas,
) -> Result<TransactionResult> {
    send(
        harness,
        governance,
        admin,
        "execute_proposal",
        ProposalIdArgs { id },
        ONE_YOCTO,
        gas,
    )
    .await
}

async fn govern(
    harness: &SandboxHarness,
    governance: &AccountId,
    admin: &AccountId,
    id: u32,
    method: &str,
    args: Value,
) -> Result<()> {
    propose(
        harness,
        governance,
        admin,
        id,
        target(method, args, ORDINARY_GAS)?,
        false,
    )
    .await?;
    successful(execute(harness, governance, admin, id, Gas::from_tgas(300)).await?);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn intermediary_governs_redstone_and_upgrades_without_access_keys(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let adapter = harness.deploy_redstone_adapter("governed-redstone").await?;
    let admin = harness.create_user("redstone-dao-admin").await?.0;
    let pending = harness.create_user("redstone-pending-owner").await?.0;

    // Exercise the actual generated WASM argument boundary, not just ConfigPatch's serde impl.
    let before_invalid = snapshot(&harness, &adapter).await?;
    let before_invalid_owner = ownership(&harness, &adapter).await?;
    for args in [
        json!({}),
        json!({"patch": null}),
        json!({"patch": {"unknown": 1}}),
        json!({"patch": [1, vec![vec![1u8; 20]], 120_000, 60_000, 0]}),
        json!({"patch": {"max_timestamp_delay_ms": "120000"}}),
    ] {
        let result = send(
            &harness,
            &adapter,
            &adapter,
            "admin_update_config",
            &args,
            ONE_YOCTO,
            ORDINARY_GAS,
        )
        .await?;
        let error = result
            .into_result()
            .expect_err("malformed patch must fail in real WASM");
        println!("RedStone rejected malformed admin_update_config {args}: {error:?}");
        let TransactionResultError::Failure(failure) = &error else {
            panic!("expected a completed failed transaction: {error:?}");
        };
        let logs = failure.logs();
        assert!(config_events(&logs).is_empty(), "{logs:?}");
        assert_eq!(snapshot(&harness, &adapter).await?, before_invalid);
        assert_eq!(ownership(&harness, &adapter).await?, before_invalid_owner);
    }

    successful(
        send(
            &harness,
            &adapter,
            &adapter,
            "write_prices",
            json!({"feed_ids": ["ETH", "BTC"], "payload": payload()}),
            NearToken::from_yoctonear(0),
            Gas::from_tgas(300),
        )
        .await?,
    );
    let seeded = snapshot(&harness, &adapter).await?;
    assert!(
        seeded["feeds"].get("ETH").is_some(),
        "ETH must actually be seeded: {seeded}"
    );
    assert!(
        seeded["feeds"].get("BTC").is_some(),
        "BTC must actually be seeded: {seeded}"
    );

    // The existing helper consumes proposal 0 for propose/accept ownership. RBAC is separate.
    let governance = harness
        .deploy_governance_contract(adapter.clone(), admin.clone())
        .await?;
    successful(
        send(
            &harness,
            &adapter,
            &adapter,
            "set_role",
            json!({"account_id": governance, "role": "ModifyRoles", "set": true}),
            ONE_YOCTO,
            ORDINARY_GAS,
        )
        .await?,
    );
    govern(
        &harness,
        &governance,
        &admin,
        1,
        "set_role",
        json!({"account_id": adapter, "role": "ModifyRoles", "set": false}),
    )
    .await?;
    let handed_over = snapshot(&harness, &adapter).await?;
    assert_eq!(handed_over["modify_roles"], json!([governance]));
    assert_eq!(handed_over["trusted_updaters"], seeded["trusted_updaters"]);
    assert_eq!(handed_over["trusted_updaters"], json!([adapter]));
    assert_eq!(
        ownership(&harness, &adapter).await?,
        json!({"owner": governance, "proposed": null})
    );

    // Both proposals exist against the same old configuration before either is dispatched.
    propose(
        &harness,
        &governance,
        &admin,
        2,
        target(
            "admin_update_config",
            json!({"patch": {"max_timestamp_delay_ms": 120_000}}),
            ORDINARY_GAS,
        )?,
        false,
    )
    .await?;
    propose(
        &harness,
        &governance,
        &admin,
        3,
        target(
            "admin_update_config",
            json!({"patch": {"max_timestamp_ahead_ms": 60_000}}),
            ORDINARY_GAS,
        )?,
        false,
    )
    .await?;
    let direct = send(
        &harness,
        &adapter,
        &admin,
        "admin_update_config",
        json!({"patch": {"max_timestamp_delay_ms": 1}}),
        ONE_YOCTO,
        ORDINARY_GAS,
    )
    .await?
    .into_result()
    .expect_err("the DAO signer is not the governance predecessor");
    assert!(format!("{direct:?}").contains("Owner only"), "{direct:?}");
    let TransactionResultError::Failure(failure) = &direct else {
        panic!("expected a completed failed transaction: {direct:?}");
    };
    let logs = failure.logs();
    assert!(config_events(&logs).is_empty(), "{logs:?}");
    assert_eq!(snapshot(&harness, &adapter).await?, handed_over);
    let executed_b =
        successful(execute(&harness, &governance, &admin, 3, Gas::from_tgas(300)).await?);
    let mut after_b = handed_over["config"].clone();
    after_b["max_timestamp_ahead_ms"] = json!(60_000);
    assert_config_event(&executed_b, &governance, &after_b);
    assert_eq!(snapshot(&harness, &adapter).await?["config"], after_b);
    let executed_a =
        successful(execute(&harness, &governance, &admin, 2, Gas::from_tgas(300)).await?);
    let merged = snapshot(&harness, &adapter).await?;
    let mut expected = handed_over.clone();
    expected["config"]["max_timestamp_delay_ms"] = json!(120_000);
    expected["config"]["max_timestamp_ahead_ms"] = json!(60_000);
    assert_eq!(
        merged, expected,
        "queued patches merge with execution-time state"
    );
    assert_config_event(&executed_a, &governance, &merged["config"]);
    println!("RedStone reverse-order queued patches: {merged}");

    revoke_all_access_keys(&harness, &adapter).await?;
    revoke_all_access_keys(&harness, &governance).await?;
    assert!(harness.view_access_keys(&adapter).await?.is_empty());
    assert!(harness.view_access_keys(&governance).await?.is_empty());
    govern(
        &harness,
        &governance,
        &admin,
        4,
        "admin_update_config",
        json!({"patch": {"min_interval_between_updates_ms": 7}}),
    )
    .await?;
    expected["config"]["min_interval_between_updates_ms"] = json!(7);
    assert_eq!(snapshot(&harness, &adapter).await?, expected);
    govern(
        &harness,
        &governance,
        &admin,
        5,
        "own_propose_owner",
        json!({"account_id": pending}),
    )
    .await?;
    let preserved = snapshot(&harness, &adapter).await?;
    let preserved_owner = ownership(&harness, &adapter).await?;
    assert_eq!(
        preserved_owner,
        json!({"owner": governance, "proposed": pending})
    );

    let code = wasm::redstone_adapter().await.to_vec();
    let published = deploy_global_contract(&harness.network, &admin, code.clone()).await?;
    let local_identity = code_hash(&harness.network, &adapter).await?;
    assert!(local_identity.contains("LocalHash"), "{local_identity}");
    propose(
        &harness,
        &governance,
        &admin,
        6,
        target(
            "admin_upgrade",
            json!({
                "code": UpgradeSource::GlobalHash(published),
                "migrate_args": Base64VecU8(b"{}".to_vec()),
            }),
            Gas::from_tgas(280),
        )?,
        false,
    )
    .await?;
    let global_outcome =
        successful(execute(&harness, &governance, &admin, 6, Gas::from_tgas(300)).await?);
    let global_identity = code_hash(&harness.network, &adapter).await?;
    assert_ne!(global_identity, local_identity);
    assert!(global_identity.contains("GlobalHash"), "{global_identity}");
    assert_eq!(snapshot(&harness, &adapter).await?, preserved);
    assert_eq!(ownership(&harness, &adapter).await?, preserved_owner);
    println!("keyless RedStone global upgrade: {local_identity} -> {global_identity}; gas {}; state {preserved}; ownership {preserved_owner}", global_outcome.total_gas_burnt);

    // A failed deploy+migrate must revert the target receipt, even though governance consumed it.
    propose(
        &harness,
        &governance,
        &admin,
        7,
        target(
            "admin_upgrade",
            json!({
                "code": UpgradeSource::Code(Base64VecU8(code.clone())),
                "migrate_args": Base64VecU8(serde_json::to_vec(&json!({"owner_id": admin}))?),
            }),
            Gas::from_tgas(400),
        )?,
        true,
    )
    .await?;
    let failed_target = execute(&harness, &governance, &admin, 7, Gas::from_tgas(500))
        .await?
        .assert_success();
    let failures = failed_target.receipt_failures();
    assert!(!failures.is_empty(), "outer success is not target success");
    let evidence = format!("{failures:#?}");
    assert!(
        evidence.contains("migrate: owner is already initialized"),
        "{evidence}"
    );
    assert_eq!(
        code_hash(&harness.network, &adapter).await?,
        global_identity
    );
    assert_eq!(snapshot(&harness, &adapter).await?, preserved);
    assert_eq!(ownership(&harness, &adapter).await?, preserved_owner);
    assert_eq!(
        view::<Value>(
            &harness.network,
            &governance,
            "get_proposal",
            ProposalIdArgs { id: 7 },
        )
        .await?,
        Value::Null,
        "successful dispatch consumes the proposal despite target failure"
    );
    println!("initialized-owner upgrade rejected in target receipt; proposal 7 consumed; code {global_identity}; failures {evidence}");

    propose(
        &harness,
        &governance,
        &admin,
        8,
        target(
            "admin_upgrade",
            json!({
                "code": UpgradeSource::Code(Base64VecU8(code)),
                "migrate_args": Base64VecU8(b"{}".to_vec()),
            }),
            Gas::from_tgas(400),
        )?,
        true,
    )
    .await?;
    let inline_outcome =
        successful(execute(&harness, &governance, &admin, 8, Gas::from_tgas(500)).await?);
    let inline_identity = code_hash(&harness.network, &adapter).await?;
    assert_eq!(
        inline_identity, local_identity,
        "identical inline bytes restore local code identity"
    );
    assert_ne!(inline_identity, global_identity);
    assert_eq!(snapshot(&harness, &adapter).await?, preserved);
    assert_eq!(ownership(&harness, &adapter).await?, preserved_owner);
    assert!(harness.view_access_keys(&adapter).await?.is_empty());
    assert!(harness.view_access_keys(&governance).await?.is_empty());
    println!("keyless RedStone inline upgrade: {global_identity} -> {inline_identity}; gas {}; preserved state {preserved}; ownership {preserved_owner}; both key lists empty", inline_outcome.total_gas_burnt);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn maximum_signer_policy_emits_complete_bounded_event(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let adapter = harness
        .deploy_redstone_adapter("max-policy-redstone")
        .await?;
    let before = snapshot(&harness, &adapter).await?;
    let signers: Vec<_> = (0u8..255)
        .map(|index| {
            let mut address = [255u8; 20];
            address[19] = index;
            address
        })
        .collect();
    let config = json!({
        "signer_count_threshold": 255,
        "signers": signers,
        "max_timestamp_delay_ms": u64::MAX,
        "max_timestamp_ahead_ms": u64::MAX,
        "min_interval_between_updates_ms": u64::MAX,
    });
    // This maximum-policy smoke uses 300 Tgas, not a claim that ordinary 50 Tgas suffices.
    let outcome = successful(
        send(
            &harness,
            &adapter,
            &adapter,
            "admin_update_config",
            json!({"patch": config}),
            ONE_YOCTO,
            Gas::from_tgas(300),
        )
        .await?,
    );
    assert_config_event(&outcome, &adapter, &config);
    let logs = outcome.logs();
    let logged: usize = logs.iter().map(|log| log.len()).sum();
    assert!(logged <= 16 * 1024, "maximum policy logged {logged} bytes");
    let mut expected = before;
    expected["config"] = config;
    assert_eq!(snapshot(&harness, &adapter).await?, expected);
    println!("RedStone config_updated emitted {logged} bytes for all 255 signers and maximum timing values");
    Ok(())
}
