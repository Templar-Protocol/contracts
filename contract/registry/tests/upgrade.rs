//! Upgrade deployed V1 registries, including the mainnet 3.1.0 release, through owner-only
//! `upgrade`. Retain the 2.0.0 in-flight callback compatibility case.

use anyhow::{Context, Result};
use near_api::types::AccountId;
use near_sdk::json_types::{Base58CryptoHash, Base64VecU8};
use near_token::NearToken;
use rstest::rstest;
use serde::Serialize;
use templar_common::{
    market::{MarketConfiguration, YieldWeights},
    registry::{RegistryEntryView, VersionAvailability, VersionInfo, VersionSource},
    upgrade::UpgradeSource,
};
use templar_gateway_core::client::registry::{
    DeployArgs, GetRegistryEntryArgs, GetVersionArgs, RemoveVersionArgs,
};
use templar_gateway_methods_spec::tx;
use templar_gateway_testing::{harness, publish_deposit_for, SandboxHarness};
use templar_gateway_types::{
    common::{ContractArgs, Pagination},
    ActionInput, Base64Bytes, ContractMethodName, ManagedAccountId, NearGas,
};

const LIVE_RELEASE: &str = "3.1.0";
const MARKET_VERSION: &str = "market@0.0.0";
const REMOVED_VERSION: &str = "market@0.0.0-removed";
/// Current source, published as a global contract so `upgrade` can name it by hash.
const SELF_VERSION: &str = "registry@self";

#[derive(Serialize)]
struct MarketInit {
    configuration: MarketConfiguration,
}

#[derive(Serialize)]
struct UpgradeArgs {
    code: UpgradeSource,
    migrate_args: Base64VecU8,
}

/// Everything an upgrade must carry over unchanged.
#[derive(Debug, PartialEq, Eq)]
struct Contents {
    versions: Vec<(String, Option<VersionInfo>)>,
    deployments: Vec<AccountId>,
}

async fn contents(harness: &SandboxHarness, registry_id: &AccountId) -> Result<Contents> {
    let client = harness.gateway_client();
    let registry = client.registry(registry_id.clone());
    let mut versions = Vec::new();
    for key in registry.list_versions(Pagination::default()).await? {
        let info = registry
            .get_version(GetVersionArgs {
                version_key: key.clone(),
            })
            .await?;
        versions.push((key, info));
    }
    Ok(Contents {
        versions,
        deployments: registry.list_deployments(Pagination::default()).await?,
    })
}

async fn market_init_args(harness: &SandboxHarness) -> Result<Vec<u8>> {
    // The market checks the configuration's shape, not that these accounts exist.
    let configuration = test_utils::market_configuration(
        harness.create_user("oracle").await?.0,
        harness.create_user("borrow").await?.0,
        harness.create_user("collateral").await?.0,
        harness.create_user("protocol").await?.0,
        YieldWeights::new_with_supply_weight(1),
    );
    Ok(serde_json::to_vec(&MarketInit { configuration })?)
}

/// A deployable version, a soft-deleted one and a finished deployment, so the upgrade has every
/// entry shape to carry.
async fn populate(harness: &SandboxHarness, registry_id: &AccountId) -> Result<()> {
    let deployer = harness.registry_signer_account_id.clone();
    for (key, code) in [
        (
            MARKET_VERSION,
            templar_gateway_testing::wasm::market().await.to_vec(),
        ),
        (REMOVED_VERSION, b"not a contract".to_vec()),
    ] {
        harness
            .registry_add_version(
                &deployer,
                registry_id,
                key,
                VersionSource::Stored(code.into()),
                NearToken::from_yoctonear(1),
            )
            .await?;
    }
    harness
        .call_function_payable(
            &deployer,
            registry_id,
            "remove_version",
            RemoveVersionArgs {
                version_key: REMOVED_VERSION.to_owned(),
            },
            NearToken::from_yoctonear(1),
        )
        .await?;

    harness
        .registry_deploy_without_abi_check(
            &deployer,
            registry_id,
            "market",
            MARKET_VERSION,
            market_init_args(harness).await?,
            None,
            NearToken::from_near(10),
        )
        .await?;
    Ok(())
}

/// Publish current source through the registry itself and return its hash.
async fn publish_current(
    harness: &SandboxHarness,
    registry_id: &AccountId,
) -> Result<Base58CryptoHash> {
    let current = templar_gateway_testing::wasm::registry().await.to_vec();
    harness
        .registry_add_version(
            &harness.registry_signer_account_id.clone(),
            registry_id,
            SELF_VERSION,
            VersionSource::PublishGlobal(current.clone().into()),
            publish_deposit_for(current.len()),
        )
        .await?;
    harness
        .gateway_client()
        .registry(registry_id.clone())
        .get_version_code_hash(GetVersionArgs {
            version_key: SELF_VERSION.to_owned(),
        })
        .await?
        .context("the global version has a code hash")
}

async fn upgrade(
    harness: &SandboxHarness,
    owner: &ManagedAccountId,
    registry_id: &AccountId,
    code: UpgradeSource,
    migrate_args: Base64VecU8,
) -> Result<()> {
    let result = harness
        .call_function_payable(
            owner,
            registry_id,
            "upgrade",
            UpgradeArgs { code, migrate_args },
            NearToken::from_yoctonear(1),
        )
        .await?;
    println!(
        "registry upgrade burnt {} Tgas",
        harness.operation_gas_burnt(&result) / 1_000_000_000_000,
    );
    Ok(())
}

async fn assert_current_state_version(
    harness: &SandboxHarness,
    registry_id: &AccountId,
) -> Result<()> {
    let version = harness.contract_state_version(registry_id).await?;
    assert_eq!(version.stored, 2);
    assert_eq!(version.target, 2);
    assert!(!version.needs_migration, "a migration is outstanding");
    Ok(())
}

#[rstest]
#[tokio::test]
async fn the_live_release_upgrades_onto_current_source(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let registry_id = harness.deploy_registry_version(LIVE_RELEASE).await?;
    populate(&harness, &registry_id).await?;
    let before = contents(&harness, &registry_id).await?;
    let hash = publish_current(&harness, &registry_id).await?;

    upgrade(
        &harness,
        &harness.registry_signer_account_id.clone(),
        &registry_id,
        UpgradeSource::GlobalHash(hash),
        Base64VecU8(br#"{"from_version":"v1"}"#.to_vec()),
    )
    .await?;

    assert_current_state_version(&harness, &registry_id).await?;
    let mut after = contents(&harness, &registry_id).await?;
    after.versions.retain(|(key, _)| key != SELF_VERSION);
    let mut before = before;
    before.versions.retain(|(key, _)| key != SELF_VERSION);
    assert_eq!(after, before);
    assert!(after
        .versions
        .iter()
        .any(|(key, info)| key == REMOVED_VERSION
            && info.as_ref().map(|info| info.availability) == Some(VersionAvailability::Removed)));

    // The upgraded registry still deploys.
    harness
        .call_function_payable(
            &harness.registry_signer_account_id.clone(),
            &registry_id,
            "deploy",
            DeployArgs {
                name: "after-upgrade".to_owned(),
                version_key: MARKET_VERSION.to_owned(),
                init_args: market_init_args(&harness).await?.into(),
                full_access_keys: None,
            },
            NearToken::from_near(10),
        )
        .await?;
    assert_eq!(contents(&harness, &registry_id).await?.deployments.len(), 2,);

    Ok(())
}

#[rstest]
#[tokio::test]
async fn upgrade_rejects_a_non_owner(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let registry_id = harness.deploy_registry_version(LIVE_RELEASE).await?;
    let stranger = harness.create_user("stranger").await?;

    let result = upgrade(
        &harness,
        &stranger,
        &registry_id,
        UpgradeSource::Code(Base64VecU8(
            templar_gateway_testing::wasm::registry().await.to_vec(),
        )),
        Base64VecU8(br#"{"from_version":"v1"}"#.to_vec()),
    )
    .await;

    // Named rather than merely failed: an `is_err` here also passes when the call never reached
    // `assert_owner` — a bad deposit, a typo'd method — which would prove nothing about access.
    let error = format!(
        "{:#}",
        result.expect_err("a non-owner upgraded the registry")
    );
    assert!(
        error.contains("Owner only"),
        "expected the owner guard to reject this, got: {error}",
    );

    Ok(())
}

/// The whole point of `upgrade`: a registry with no access keys left can still be replaced.
///
/// Ownership has to move off the registry first: `new` makes the registry its own owner, so
/// revoking its keys without handing ownership over would strand `upgrade` behind a signer that no
/// longer exists.
#[rstest]
#[tokio::test]
async fn a_keyless_registry_still_upgrades_through_its_owner(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let registry_id = harness.deploy_registry_version(LIVE_RELEASE).await?;
    populate(&harness, &registry_id).await?;
    let hash = publish_current(&harness, &registry_id).await?;
    let before = contents(&harness, &registry_id).await?;

    let new_owner = harness.create_user("registry-owner").await?;
    harness
        .transfer_ownership(
            &registry_id,
            &harness.registry_signer_account_id.clone(),
            &new_owner,
        )
        .await?;
    harness.revoke_all_access_keys(&registry_id).await?;
    assert!(harness.view_access_keys(&registry_id).await?.is_empty());

    upgrade(
        &harness,
        &new_owner,
        &registry_id,
        UpgradeSource::GlobalHash(hash),
        Base64VecU8(br#"{"from_version":"v1"}"#.to_vec()),
    )
    .await?;

    assert_current_state_version(&harness, &registry_id).await?;
    assert_eq!(contents(&harness, &registry_id).await?, before);

    Ok(())
}

/// A deploy that 2.0.0 starts but the new code finishes: its finalize callback carries 2.0.0's
/// argument names, so it must still record the deployment rather than strand the name `Reserved`.
#[rstest]
#[tokio::test]
async fn a_deploy_in_flight_across_the_upgrade_still_finalizes(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let registry_id = harness.deploy_registry_version("2.0.0").await?;
    populate(&harness, &registry_id).await?;
    let in_flight: AccountId = format!("in-flight.{registry_id}").parse()?;

    // One receipt: the deploy enqueues its callback, then the code under it is replaced.
    harness
        .execute(
            &harness.registry_signer_account_id.clone(),
            tx::Batch {
                receiver_id: registry_id.clone(),
                actions: vec![
                    ActionInput::FunctionCall {
                        method_name: ContractMethodName("deploy".to_owned()),
                        args: ContractArgs::Json(serde_json::to_value(DeployArgs {
                            name: "in-flight".to_owned(),
                            version_key: MARKET_VERSION.to_owned(),
                            init_args: market_init_args(&harness).await?.into(),
                            full_access_keys: None,
                        })?),
                        gas: NearGas::from_tgas(200),
                        deposit: NearToken::from_near(10),
                    },
                    ActionInput::DeployContract {
                        code: Base64Bytes(templar_gateway_testing::wasm::registry().await.to_vec()),
                    },
                    ActionInput::FunctionCall {
                        method_name: ContractMethodName("migrate".to_owned()),
                        args: ContractArgs::Json(serde_json::json!({"from_version": "v1"})),
                        gas: NearGas::from_tgas(20),
                        deposit: NearToken::ZERO,
                    },
                ],
            },
        )
        .await?;
    assert_current_state_version(&harness, &registry_id).await?;

    assert_eq!(
        harness.code_hash(&registry_id).await?,
        near_api::types::CryptoHash::hash(templar_gateway_testing::wasm::registry().await)
            .to_string(),
        "the batch must have replaced the code",
    );
    assert!(
        matches!(
            harness
                .gateway_client()
                .registry(registry_id.clone())
                .get_registry_entry(GetRegistryEntryArgs {
                    account_id: in_flight,
                })
                .await?,
            Some(RegistryEntryView::Deployed(_)),
        ),
        "the callback 2.0.0 enqueued must finalize on the new code",
    );

    Ok(())
}

/// A failing migration shares the deployment receipt, so neither the new code nor its state
/// writes may survive. Read the old registry and deploy again to prove it remains usable.
#[rstest]
#[tokio::test]
async fn a_failed_migration_preserves_the_released_code_and_state(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let registry_id = harness.deploy_registry_version(LIVE_RELEASE).await?;
    populate(&harness, &registry_id).await?;
    let hash = publish_current(&harness, &registry_id).await?;
    let before = contents(&harness, &registry_id).await?;
    let old_code = harness.code_hash(&registry_id).await?;
    let old_version = harness.contract_state_version(&registry_id).await?;
    assert_eq!(old_version.stored, 1);
    assert_eq!(old_version.target, 1);
    assert!(!old_version.needs_migration);
    let owner: Option<AccountId> = harness
        .view_json(&registry_id, "own_get_owner", serde_json::json!({}))
        .await?;

    let result = upgrade(
        &harness,
        &harness.registry_signer_account_id.clone(),
        &registry_id,
        UpgradeSource::GlobalHash(hash),
        Base64VecU8(br#"{"from_version":"unsupported"}"#.to_vec()),
    )
    .await;
    let error = format!("{:#}", result.expect_err("invalid migration succeeded"));
    assert!(
        error.contains("unknown variant") && error.contains("unsupported"),
        "the migration decoder must reject the input: {error}",
    );
    assert_eq!(harness.code_hash(&registry_id).await?, old_code);
    let after_version = harness.contract_state_version(&registry_id).await?;
    assert_eq!(after_version.stored, old_version.stored);
    assert_eq!(after_version.target, old_version.target);
    assert_eq!(after_version.needs_migration, old_version.needs_migration);
    assert_eq!(contents(&harness, &registry_id).await?, before);
    assert_eq!(
        harness
            .view_json::<Option<AccountId>>(&registry_id, "own_get_owner", serde_json::json!({}),)
            .await?,
        owner,
    );
    // The original implementation can still finalize a deployment after the rolled-back upgrade.
    harness
        .registry_deploy_without_abi_check(
            &harness.registry_signer_account_id.clone(),
            &registry_id,
            "after-failed-upgrade",
            MARKET_VERSION,
            market_init_args(&harness).await?,
            None,
            NearToken::from_near(10),
        )
        .await?;
    assert_eq!(contents(&harness, &registry_id).await?.deployments.len(), 2);
    Ok(())
}

#[rstest]
#[tokio::test]
async fn a_v2_same_code_upgrade_preserves_uploads_roles_and_registry_state(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let registry_id = harness.deploy_registry_version(LIVE_RELEASE).await?;
    populate(&harness, &registry_id).await?;
    let hash = publish_current(&harness, &registry_id).await?;
    let owner = harness.registry_signer_account_id.clone();
    upgrade(
        &harness,
        &owner,
        &registry_id,
        UpgradeSource::GlobalHash(hash),
        Base64VecU8(br#"{"from_version":"v1"}"#.to_vec()),
    )
    .await?;
    let operator = harness.create_user("operator").await?;
    for role in ["Uploader", "Deployer"] {
        harness
            .call_function_payable(
                &owner,
                &registry_id,
                "set_role",
                serde_json::json!({"account_id": operator.0, "role": role, "set": true}),
                NearToken::from_yoctonear(1),
            )
            .await?;
    }
    harness
        .call_function_payable(
            &operator,
            &registry_id,
            "storage_deposit",
            serde_json::json!({}),
            NearToken::from_near(1),
        )
        .await?;
    let upload = vec![0_u8, 1, 2, 255];
    let upload_hash = Base58CryptoHash::from(near_api::types::CryptoHash::hash(&upload).0);
    harness
        .call_function_borsh(
            &operator,
            &registry_id,
            "upload_code",
            upload,
            NearToken::ZERO,
        )
        .await?;
    let retained = vec![9_u8, 8, 7, 6];
    let retained_hash = Base58CryptoHash::from(near_api::types::CryptoHash::hash(&retained).0);
    harness
        .call_function_borsh(
            &operator,
            &registry_id,
            "upload_code",
            retained,
            NearToken::ZERO,
        )
        .await?;
    harness
        .call_function_payable(
            &owner,
            &registry_id,
            "approve_version",
            serde_json::json!({"version_key": "retained", "code": {"Uploaded": retained_hash}}),
            NearToken::from_yoctonear(1),
        )
        .await?;
    harness
        .call_function_payable(
            &owner,
            &registry_id,
            "own_propose_owner",
            serde_json::json!({"account_id": operator.0}),
            NearToken::from_yoctonear(1),
        )
        .await?;

    let views = [
        ("own_get_owner", serde_json::json!({})),
        ("own_get_proposed_owner", serde_json::json!({})),
        ("get_upload", serde_json::json!({"code_hash": upload_hash})),
        ("list_uploads", serde_json::json!({})),
        ("list_role", serde_json::json!({"role": "Uploader"})),
        ("list_role", serde_json::json!({"role": "Deployer"})),
        (
            "has_role",
            serde_json::json!({"account_id": operator.0, "role": "Uploader"}),
        ),
        (
            "has_role",
            serde_json::json!({"account_id": operator.0, "role": "Deployer"}),
        ),
        (
            "storage_balance_of",
            serde_json::json!({"account_id": operator.0}),
        ),
    ];
    let mut before_views = Vec::new();
    for (method, args) in &views {
        before_views.push(
            harness
                .view_json::<serde_json::Value>(&registry_id, method, args)
                .await?,
        );
    }
    assert!(
        !before_views[2].is_null(),
        "upload must exist before upgrade"
    );
    assert_eq!(before_views[6], serde_json::json!(true));
    assert_eq!(before_views[7], serde_json::json!(true));
    let before = contents(&harness, &registry_id).await?;
    let before_code = harness.code_hash(&registry_id).await?;
    upgrade(
        &harness,
        &owner,
        &registry_id,
        UpgradeSource::GlobalHash(hash),
        Base64VecU8(Vec::new()),
    )
    .await?;
    assert_current_state_version(&harness, &registry_id).await?;
    assert_eq!(harness.code_hash(&registry_id).await?, before_code);
    assert_eq!(contents(&harness, &registry_id).await?, before);
    for ((method, args), expected) in views.iter().zip(before_views) {
        assert_eq!(
            harness
                .view_json::<serde_json::Value>(&registry_id, method, args)
                .await?,
            expected,
            "{method} changed across a same-version upgrade",
        );
    }
    let before_removal: serde_json::Value = harness
        .view_json(
            &registry_id,
            "storage_balance_of",
            serde_json::json!({"account_id": operator.0}),
        )
        .await?;
    harness
        .call_function_payable(
            &owner,
            &registry_id,
            "remove_version",
            serde_json::json!({"version_key": "retained"}),
            NearToken::from_yoctonear(1),
        )
        .await?;
    let after_removal: serde_json::Value = harness
        .view_json(
            &registry_id,
            "storage_balance_of",
            serde_json::json!({"account_id": operator.0}),
        )
        .await?;
    assert_eq!(before_removal["total"], after_removal["total"]);
    let available = |balance: &serde_json::Value| -> Result<u128> {
        Ok(balance["available"]
            .as_str()
            .context("available balance")?
            .parse()?)
    };
    assert!(available(&after_removal)? > available(&before_removal)?);
    Ok(())
}
