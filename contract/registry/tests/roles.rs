//! Actual Wasm authorization and DAO-as-predecessor evidence. The mock DAO does not model
//! production council policy or voting thresholds.

use anyhow::{Context, Result};
use near_api::types::AccountId;
use near_sdk::json_types::Base64VecU8;
use near_token::NearToken;
use serde_json::{json, Value};
use templar_common::{market::YieldWeights, registry::VersionSource};
use templar_gateway_methods_spec::tx;
use templar_gateway_testing::harness;
use templar_gateway_testing::{failed_receipts, publish_deposit_for, SandboxHarness};
use templar_gateway_types::Base64Bytes;
use templar_gateway_types::{
    common::{ContractArgs, WriteOperationResult},
    ContractMethodName, ManagedAccountId, NearGas,
};

const KEY: &str = "ed25519:5BGSaf6YjVm7565VzWQHNxoyEjwr3jUpRJSGjREvU9dB";
const ONE: NearToken = NearToken::from_yoctonear(1);

fn operation(id: &AccountId, method: &str, args: Value, deposit: NearToken) -> tx::FunctionCall {
    tx::FunctionCall {
        receiver_id: id.clone(),
        method_name: ContractMethodName(method.to_owned()),
        args: ContractArgs::Json(args),
        gas: NearGas::from_tgas(300),
        deposit,
    }
}

fn assert_rejected(result: &WriteOperationResult, message: &str) {
    assert!(
        failed_receipts(result).next().is_some(),
        "expected receipt rejection: {message}"
    );
    if !message.is_empty() {
        let Some(reason) = result.operation.failure_message() else {
            panic!("rejection has no failure evidence");
        };
        assert!(reason.contains(message), "expected {message:?}: {reason}");
    }
}

async fn view(h: &SandboxHarness, id: &AccountId, method: &str, args: Value) -> Result<Value> {
    h.view_json(id, method, args).await
}

async fn change_role(
    h: &SandboxHarness,
    owner: &ManagedAccountId,
    registry: &AccountId,
    account: &ManagedAccountId,
    role: &str,
) -> Result<()> {
    h.call_function_payable(
        owner,
        registry,
        "grant_role",
        json!({
            "account_id": account.0, "role": role,
        }),
        ONE,
    )
    .await?;
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "one FunctionCall proposal carries target, payload, gas and expected outcome"
)]
async fn proposal(
    h: &SandboxHarness,
    proposer: &ManagedAccountId,
    dao: &AccountId,
    registry: &AccountId,
    method: &str,
    args: Value,
    gas: u64,
    succeeds: bool,
) -> Result<()> {
    let kind = json!({"FunctionCall": {"receiver_id": registry, "actions": [{
        "method_name": method,
        "args": Base64VecU8(serde_json::to_vec(&args)?),
        "deposit": "1", "gas": (gas * 1_000_000_000_000).to_string(),
    }]}});
    let id: u64 = h
        .call_function_json(
            proposer,
            dao,
            "add_proposal",
            json!({
                "proposal": {"description": format!("ENG-830 {method}"), "kind": kind},
            }),
        )
        .await?;
    let stored = view(h, dao, "get_proposal", json!({"id": id})).await?;
    assert_eq!(stored["kind"], kind);
    let op = operation(
        dao,
        "act_proposal",
        json!({
            "id": id, "action": "VoteApprove", "proposal": stored["kind"], "memo": null,
        }),
        NearToken::ZERO,
    );
    if succeeds {
        h.execute(proposer, op).await?;
    } else {
        let outcome = h.try_execute(proposer, op).await?;
        assert_rejected(&outcome, "");
    }
    let completed = view(h, dao, "get_proposal", json!({"id": id})).await?;
    assert_eq!(
        completed["status"],
        if succeeds { "Approved" } else { "Failed" }
    );
    println!("DAO proposal {id} {method}: {}", completed["status"]);
    Ok(())
}

async fn upload(
    h: &SandboxHarness,
    uploader: &ManagedAccountId,
    registry: &AccountId,
    code: &[u8],
) -> Result<String> {
    let hash = near_api::types::CryptoHash::hash(code).to_string();
    let len = code.len();
    h.call_function_borsh(
        uploader,
        registry,
        "upload_code",
        code,
        NearToken::from_near(30),
    )
    .await?;
    let metadata = view(h, registry, "get_upload", json!({"code_hash": hash})).await?;
    assert_eq!(metadata, json!({"uploader": uploader.0, "code_len": len}));
    println!("uploaded {hash}: {metadata}");
    Ok(hash)
}

async fn state_snapshot(h: &SandboxHarness, registry: &AccountId) -> Result<Value> {
    let mut snapshot = serde_json::Map::new();
    for method in [
        "own_get_owner",
        "own_get_proposed_owner",
        "list_versions",
        "list_uploads",
        "list_deployments",
        "get_stored_state_version",
        "get_target_state_version",
        "needs_migration",
    ] {
        snapshot.insert(
            method.to_owned(),
            view(h, registry, method, json!({})).await?,
        );
    }
    for role in ["Uploader", "Deployer"] {
        snapshot.insert(
            role.to_owned(),
            view(h, registry, "list_role_members", json!({"role": role})).await?,
        );
    }
    assert_eq!(snapshot["get_stored_state_version"], 2);
    assert_eq!(snapshot["get_target_state_version"], 2);
    assert_eq!(snapshot["needs_migration"], false);
    let keys = snapshot["list_versions"]
        .as_array()
        .context("version list")?
        .clone();
    for key in keys {
        snapshot.insert(
            format!("version:{key}"),
            view(h, registry, "get_version", json!({"version_key": key})).await?,
        );
    }
    let deployments = snapshot["list_deployments"]
        .as_array()
        .context("deployment list")?
        .clone();
    for account in deployments {
        snapshot.insert(
            format!("deployment:{account}"),
            view(
                h,
                registry,
                "get_registry_entry",
                json!({"account_id": account}),
            )
            .await?,
        );
    }
    Ok(Value::Object(snapshot))
}

async fn deploy_approved(
    h: &SandboxHarness,
    deployer: &ManagedAccountId,
    registry: &AccountId,
    name: &str,
    version: &str,
    hash: &str,
    init: Value,
) -> Result<AccountId> {
    h.execute(
        deployer,
        operation(
            registry,
            "deploy",
            json!({
                "name": name, "version_key": version,
                "init_args": Base64VecU8(serde_json::to_vec(&init)?), "full_access_keys": [KEY],
            }),
            NearToken::from_near(10),
        ),
    )
    .await?;
    let child: AccountId = format!("{name}.{registry}").parse()?;
    assert_eq!(h.code_hash(&child).await?, hash);
    let keys = h.view_access_keys(&child).await?;
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].0, KEY);
    assert!(keys[0].1);
    let record = view(h, registry, "get_deployment", json!({"account_id": child})).await?;
    assert_eq!(record["version_key"], version);
    assert_eq!(record["code_hash"], hash);
    println!("child {child}: hash={hash}, record={record}, keys={keys:?}");
    Ok(child)
}

/// Every write is a real sandbox transaction.
#[rstest::rstest]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "ordered DAO ownership and key-removal proof shares one sandbox"
)]
async fn dao_approval_and_keyless_upgrade(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let h = &harness;
    let registry = h.deploy_registry().await?;
    let owner = h.registry_signer_account_id.clone();
    let dao = h.deploy_mock_dao("registry-dao", NearToken::ZERO).await?;
    let uploader = h.create_user("uploader").await?;
    let deployer = h.create_user("deployer").await?;
    let proposer = h.create_user("proposer").await?;
    let unrelated = h.create_user("unrelated").await?;
    change_role(h, &owner, &registry, &uploader, "Uploader").await?;
    change_role(h, &owner, &registry, &deployer, "Deployer").await?;
    let current = templar_gateway_testing::wasm::registry().await.to_vec();
    let current_hash = near_api::types::CryptoHash::hash(&current).to_string();
    let publish_deposit = publish_deposit_for(current.len());
    h.registry_add_version(
        &owner,
        &registry,
        "registry@self",
        VersionSource::PublishGlobal(current.into()),
        publish_deposit,
    )
    .await?;
    let hash = upload(
        h,
        &uploader,
        &registry,
        templar_gateway_testing::wasm::mock_dao().await,
    )
    .await?;
    let approval = json!({"version_key": "dao@approved", "code": {"Uploaded": hash}});
    let before = state_snapshot(h, &registry).await?;
    proposal(
        h,
        &proposer,
        &dao,
        &registry,
        "approve_version",
        approval.clone(),
        80,
        false,
    )
    .await?;
    assert_eq!(state_snapshot(h, &registry).await?, before);
    assert!(
        !view(h, &registry, "get_upload", json!({"code_hash": hash}))
            .await?
            .is_null()
    );
    h.call_function_payable(
        &owner,
        &registry,
        "own_propose_owner",
        json!({"account_id": dao}),
        ONE,
    )
    .await?;
    proposal(
        h,
        &proposer,
        &dao,
        &registry,
        "own_accept_owner",
        json!({}),
        30,
        true,
    )
    .await?;
    assert_eq!(
        view(h, &registry, "own_get_owner", json!({})).await?,
        json!(dao)
    );
    assert_eq!(
        view(h, &registry, "own_get_proposed_owner", json!({})).await?,
        Value::Null
    );
    println!("registry owner: {dao}");
    proposal(
        h,
        &proposer,
        &dao,
        &registry,
        "approve_version",
        approval,
        80,
        true,
    )
    .await?;
    assert_eq!(
        view(h, &registry, "get_upload", json!({"code_hash": hash})).await?,
        Value::Null
    );
    let approved = view(
        h,
        &registry,
        "get_version",
        json!({"version_key": "dao@approved"}),
    )
    .await?;
    assert_eq!(approved["code_hash"], hash);
    println!("approved version: {approved}");
    deploy_approved(
        h,
        &deployer,
        &registry,
        "dao-child",
        "dao@approved",
        &hash,
        json!({"proposal_bond": "0"}),
    )
    .await?;
    let upgrade = json!({"code": {"GlobalHash": current_hash}, "migrate_args": ""});
    for actor in [&owner, &uploader, &deployer, &unrelated] {
        for (method, args) in [
            (
                "approve_version",
                json!({"version_key": "unauthorized", "code": {"Global": current_hash}}),
            ),
            ("upgrade", upgrade.clone()),
        ] {
            let result = h
                .try_execute(actor, operation(&registry, method, args, ONE))
                .await?;
            assert_rejected(&result, "Owner");
        }
    }
    assert_eq!(
        view(
            h,
            &registry,
            "has_role",
            json!({"account_id": owner.0, "role": "Uploader"})
        )
        .await?,
        false
    );
    assert_eq!(
        view(
            h,
            &registry,
            "has_role",
            json!({"account_id": uploader.0, "role": "Uploader"})
        )
        .await?,
        true
    );
    for (method, expected) in [("grant_role", true), ("revoke_role", false)] {
        proposal(
            h,
            &proposer,
            &dao,
            &registry,
            method,
            json!({"account_id": unrelated.0, "role": "Deployer"}),
            30,
            true,
        )
        .await?;
        assert_eq!(
            view(
                h,
                &registry,
                "has_role",
                json!({"account_id": unrelated.0, "role": "Deployer"})
            )
            .await?,
            expected
        );
    }
    // Keep a pending upload across both same-code upgrades, in addition to grants and deployments.
    let pending = upload(h, &uploader, &registry, b"pending across upgrades").await?;
    for keyless in [false, true] {
        if keyless {
            h.revoke_all_access_keys(&registry).await?;
        }
        let keys_before = h.view_access_keys(&registry).await?;
        if keyless {
            assert!(keys_before.is_empty());
        } else {
            assert!(!keys_before.is_empty());
        }
        let snapshot = state_snapshot(h, &registry).await?;
        let pending_before =
            view(h, &registry, "get_upload", json!({"code_hash": pending})).await?;
        println!("before upgrade keyless={keyless}: {snapshot}, keys={keys_before:?}");
        proposal(
            h,
            &proposer,
            &dao,
            &registry,
            "upgrade",
            upgrade.clone(),
            280,
            true,
        )
        .await?;
        let account = h.gateway_client().account().get(registry.clone()).await?;
        assert_eq!(
            account.contract_state,
            near_api::types::account::ContractState::GlobalHash(current_hash.parse()?)
        );
        println!("registry code: {:?}", account.contract_state);
        assert_eq!(state_snapshot(h, &registry).await?, snapshot);
        assert_eq!(
            view(h, &registry, "get_upload", json!({"code_hash": pending})).await?,
            pending_before
        );
        assert_eq!(h.view_access_keys(&registry).await?, keys_before);
        println!(
            "after upgrade keyless={keyless}: {}",
            state_snapshot(h, &registry).await?
        );
    }
    let market_hash = upload(
        h,
        &uploader,
        &registry,
        templar_gateway_testing::wasm::market().await,
    )
    .await?;
    assert_ne!(market_hash, hash);
    proposal(
        h,
        &proposer,
        &dao,
        &registry,
        "approve_version",
        json!({"version_key": "market@keyless", "code": {"Uploaded": market_hash}}),
        80,
        true,
    )
    .await?;
    assert_eq!(
        view(
            h,
            &registry,
            "get_upload",
            json!({"code_hash": market_hash})
        )
        .await?,
        Value::Null
    );
    let configuration = test_utils::market_configuration(
        h.create_user("oracle").await?.0,
        h.create_user("borrow").await?.0,
        h.create_user("collateral").await?.0,
        h.create_user("protocol").await?.0,
        YieldWeights::new_with_supply_weight(1),
    );
    let market = deploy_approved(
        h,
        &deployer,
        &registry,
        "market-child",
        "market@keyless",
        &market_hash,
        json!({"configuration": configuration}),
    )
    .await?;
    assert_eq!(h.get_configuration(&market).await?, configuration);
    assert!(h.view_access_keys(&registry).await?.is_empty());
    let result = h
        .try_call_function(&unrelated, &registry, "own_renounce_owner", json!({}))
        .await?;
    assert_rejected(&result, "MethodNotFound");
    Ok(())
}

#[rstest::rstest]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "actual wrapper matrix keeps identities on the same initialized registry"
)]
async fn actual_wasm_authorization_wrappers(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let h = &harness;
    let owner = h.registry_signer_account_id.clone();
    let registry = h.create_user("wrapper-registry").await?.0;
    h.deploy_code(
        &registry,
        templar_gateway_testing::wasm::registry().await.to_vec(),
    )
    .await?;
    h.call_function(&owner, &registry, "new", json!({})).await?;
    let uploader = h.create_user("uploader").await?;
    let deployer = h.create_user("deployer").await?;
    let both = h.create_user("both").await?;
    let unrelated = h.create_user("unrelated").await?;
    for (actor, role) in [
        (&uploader, "Uploader"),
        (&deployer, "Deployer"),
        (&both, "Uploader"),
        (&both, "Deployer"),
    ] {
        change_role(h, &owner, &registry, actor, role).await?;
    }
    assert_eq!(
        view(h, &registry, "own_get_owner", json!({})).await?,
        json!(owner.0)
    );
    let code = templar_gateway_testing::wasm::mock_dao().await.to_vec();
    let code_hash = near_api::types::CryptoHash::hash(&code).to_string();
    h.registry_add_version(
        &owner,
        &registry,
        "wrapper@dao",
        VersionSource::Stored(code.clone().into()),
        ONE,
    )
    .await?;
    let identities = [&owner, &uploader, &deployer, &both, &unrelated];
    for (index, actor) in identities.into_iter().enumerate() {
        let result = h
            .try_call_function(actor, &registry, "new", json!({}))
            .await?;
        assert_rejected(&result, "initialized");
        let result = h
            .try_call_function(actor, &registry, "own_renounce_owner", json!({}))
            .await?;
        assert_rejected(&result, "MethodNotFound");
        for (method, args) in [
            ("migrate", json!({})),
            (
                "add_version_01_finalize",
                json!({"version_key": "wrapper@dao"}),
            ),
            (
                "deploy_01_finalize",
                json!({"account_id": registry, "market_id": null,
                "deployment": {"version_key": "wrapper@dao", "code_hash": code_hash, "block_height": "0"}}),
            ),
            ("fail", json!({"message": "external must not execute this"})),
        ] {
            let result = h.try_call_function(actor, &registry, method, args).await?;
            assert_rejected(&result, "private");
        }
        let bytes = vec![u8::try_from(index)?, 3, 2, 1];
        let hash = near_api::types::CryptoHash::hash(&bytes).to_string();
        let upload_op = tx::FunctionCall {
            args: ContractArgs::Raw(Base64Bytes(near_sdk::borsh::to_vec(&bytes)?)),
            ..operation(
                &registry,
                "upload_code",
                Value::Null,
                NearToken::from_near(1),
            )
        };
        if matches!(index, 0 | 1 | 3) {
            h.execute(actor, upload_op).await?;
            h.call_function_payable(
                actor,
                &registry,
                "remove_upload",
                json!({"code_hash": hash}),
                ONE,
            )
            .await?;
        } else {
            let result = h.try_execute(actor, upload_op).await?;
            assert_rejected(&result, "Unauthorized role");
        }
        for name in ["probe", "deploy"] {
            let child: AccountId = format!("{name}.{registry}").parse()?;
            let before = view(
                h,
                &registry,
                "get_registry_entry",
                json!({"account_id": child}),
            )
            .await?;
            assert!(before.is_null());
            let result = h
                .try_execute(
                    actor,
                    operation(
                        &registry,
                        "deploy",
                        json!({
                            "name": name, "version_key": "wrapper@dao",
                            "init_args": Base64VecU8(br#"{"proposal_bond":"0"}"#.to_vec()),
                            "full_access_keys": [KEY],
                        }),
                        NearToken::from_near(5),
                    ),
                )
                .await?;
            assert_rejected(
                &result,
                if matches!(index, 0 | 2 | 3) {
                    "Deployment name is reserved"
                } else {
                    "Unauthorized role"
                },
            );
            assert_eq!(
                view(
                    h,
                    &registry,
                    "get_registry_entry",
                    json!({"account_id": child}),
                )
                .await?,
                before
            );
            assert!(h.gateway_client().account().get(child).await.is_err());
        }
        let deploy = operation(
            &registry,
            "deploy",
            json!({
                "name": format!("wrapper-{index}"), "version_key": "wrapper@dao",
                "init_args": Base64VecU8(br#"{"proposal_bond":"0"}"#.to_vec()), "full_access_keys": [KEY],
            }),
            NearToken::from_near(5),
        );
        if matches!(index, 0 | 2 | 3) {
            h.execute(actor, deploy).await?;
            let child: AccountId = format!("wrapper-{index}.{registry}").parse()?;
            assert_eq!(h.code_hash(&child).await?, code_hash);
            assert_eq!(
                h.view_access_keys(&child).await?,
                vec![(KEY.to_owned(), true)]
            );
            assert!(
                !view(h, &registry, "get_deployment", json!({"account_id": child}))
                    .await?
                    .is_null()
            );
        } else {
            let result = h.try_execute(actor, deploy).await?;
            assert_rejected(&result, "Unauthorized role");
        }
        if index != 0 {
            let result = h
                .try_execute(
                    actor,
                    operation(
                        &registry,
                        "own_propose_owner",
                        json!({"account_id": unrelated.0}),
                        ONE,
                    ),
                )
                .await?;
            assert_rejected(&result, "Owner only");
        }
    }
    // Reserved-name rejections must leave the implementation's scratch accounts usable.
    h.registry_add_version(
        &owner,
        &registry,
        "wrapper@published",
        VersionSource::PublishGlobal(code.clone().into()),
        publish_deposit_for(code.len()),
    )
    .await?;
    h.registry_add_version(
        &owner,
        &registry,
        "wrapper@global",
        VersionSource::ExistingGlobal(code_hash.parse()?),
        ONE,
    )
    .await?;
    for version in ["wrapper@published", "wrapper@global"] {
        assert_eq!(
            view(h, &registry, "get_version", json!({"version_key": version})).await?["code_hash"],
            code_hash
        );
    }
    for name in ["probe", "deploy"] {
        let scratch: AccountId = format!("{name}.{registry}").parse()?;
        assert!(h.gateway_client().account().get(scratch).await.is_err());
    }
    // Views are called as transactions so the five distinct predecessors are exercised.
    for (method, args) in [
        ("own_get_owner", json!({})),
        ("own_get_proposed_owner", json!({})),
        ("get_stored_state_version", json!({})),
        ("get_target_state_version", json!({})),
        ("needs_migration", json!({})),
        ("list_versions", json!({})),
        ("list_deployments", json!({})),
        ("list_uploads", json!({})),
        ("list_role_members", json!({"role": "Uploader"})),
        (
            "has_role",
            json!({"account_id": uploader.0, "role": "Uploader"}),
        ),
        ("get_upload", json!({"code_hash": code_hash})),
        ("get_version", json!({"version_key": "wrapper@dao"})),
        (
            "get_version_code_hash",
            json!({"version_key": "wrapper@dao"}),
        ),
        ("get_registry_entry", json!({"account_id": unrelated.0})),
        ("get_deployment", json!({"account_id": unrelated.0})),
    ] {
        let expected = view(h, &registry, method, args.clone()).await?;
        for actor in identities {
            let actual: Value = h
                .call_function_json(actor, &registry, method, args.clone())
                .await?;
            assert_eq!(actual, expected, "{method}: {}", actor.0);
        }
    }
    // Confirmation deposits are checked by the actual exported wrappers, before domain guards.
    for deposit in [NearToken::ZERO, NearToken::from_yoctonear(2)] {
        for (method, args) in [
            (
                "grant_role",
                json!({"account_id": unrelated.0, "role": "Uploader"}),
            ),
            (
                "revoke_role",
                json!({"account_id": uploader.0, "role": "Uploader"}),
            ),
            ("own_propose_owner", json!({"account_id": unrelated.0})),
            ("own_accept_owner", json!({})),
            ("remove_upload", json!({"code_hash": code_hash})),
            (
                "approve_version",
                json!({"version_key": "unused", "code": {"Uploaded": code_hash}}),
            ),
            ("remove_version", json!({"version_key": "wrapper@dao"})),
            (
                "upgrade",
                json!({"code": {"GlobalHash": code_hash}, "migrate_args": ""}),
            ),
        ] {
            let result = h
                .try_execute(&owner, operation(&registry, method, args, deposit))
                .await?;
            assert_rejected(&result, "exactly 1 yocto");
        }
        let args =
            near_sdk::borsh::to_vec(&("unused".to_owned(), VersionSource::Stored(vec![1].into())))?;
        let result = h
            .try_execute(
                &owner,
                tx::FunctionCall {
                    args: ContractArgs::Raw(Base64Bytes(args)),
                    ..operation(&registry, "add_version", Value::Null, deposit)
                },
            )
            .await?;
        assert_rejected(&result, "exactly 1 yocto");
    }
    h.call_function_payable(
        &owner,
        &registry,
        "own_propose_owner",
        json!({"account_id": unrelated.0}),
        ONE,
    )
    .await?;
    h.call_function_payable(
        &owner,
        &registry,
        "own_propose_owner",
        json!({"account_id": null}),
        ONE,
    )
    .await?;
    assert_eq!(
        view(h, &registry, "own_get_proposed_owner", json!({})).await?,
        Value::Null
    );
    h.call_function_payable(
        &owner,
        &registry,
        "own_propose_owner",
        json!({"account_id": unrelated.0}),
        ONE,
    )
    .await?;
    for actor in [&owner, &uploader, &deployer, &both] {
        let result = h
            .try_execute(
                actor,
                operation(&registry, "own_accept_owner", json!({}), ONE),
            )
            .await?;
        assert_rejected(&result, "Proposed owner only");
    }
    h.call_function_payable(&unrelated, &registry, "own_accept_owner", json!({}), ONE)
        .await?;
    assert_eq!(
        view(h, &registry, "own_get_owner", json!({})).await?,
        json!(unrelated.0)
    );
    assert_eq!(
        view(h, &registry, "own_get_proposed_owner", json!({})).await?,
        Value::Null
    );
    Ok(())
}
