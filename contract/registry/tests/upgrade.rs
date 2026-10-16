//! Upgrade a registry through its owner-only `upgrade`, starting from 2.0.0 — the release every
//! live registry runs — so a change that cannot carry one of them fails here rather than on chain.

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

const LIVE_RELEASE: &str = "2.0.0";
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
) -> Result<()> {
    let result = harness
        .call_function_payable(
            owner,
            registry_id,
            "upgrade",
            UpgradeArgs {
                code,
                migrate_args: Base64VecU8(Vec::new()),
            },
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
    let client = harness.gateway_client();
    let contract = client.contract(registry_id.clone());
    anyhow::ensure!(
        !contract.needs_migration(()).await?,
        "a migration is outstanding"
    );
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
    let registry_id = harness.deploy_registry_version(LIVE_RELEASE).await?;
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
                ],
            },
        )
        .await?;

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
