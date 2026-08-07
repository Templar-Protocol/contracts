//! Bring each live registry release onto the current one.
//!
//! `templar-alpha.near`, `v1.tmplr.near` and `user0.tmplr.near` run 0.1.0, 1.0.0 and 1.1.0
//! respectively, spanning both pre-versioning state layouts, and none of them wrote a state
//! version. Each case starts from that release's real binary, builds state through *its* API, and
//! upgrades onto current source — so a migration that cannot carry one of them fails here rather
//! than on the account.

use anyhow::{Context, Result};
use near_api::types::AccountId;
use near_token::NearToken;
use rstest::rstest;
use templar_common::{
    market::YieldWeights,
    registry::{DeployMode, VersionAvailability},
};
use templar_gateway_testing::{harness, SandboxHarness};

const MARKET_VERSION: &str = "market@0.0.0";
const REMOVED_VERSION: &str = "market@0.0.0-removed";
/// The registry's own code, republished as a global contract so `upgrade` can name it by hash.
const SELF_VERSION: &str = "registry@self";

/// Which layout a release stores, and therefore the migration it needs. Both report a stored
/// version of 0, so this mapping is knowledge the operator supplies — nothing on-chain reveals it.
fn migration_for(release: &str) -> serde_json::Value {
    let from_version = match release {
        "0.1.0" | "1.0.0" => "pre_global_contracts",
        "1.1.0" => "with_global_contracts",
        other => panic!("no migration mapped for registry {other}"),
    };
    serde_json::json!({ "from_version": from_version })
}

struct Populated {
    market_id: AccountId,
    code_hash: String,
}

/// Register a deployable version and a soft-deleted one, then deploy a market from the first, so
/// the migration has a `versions` map holding both entry shapes and a `registry` map holding a
/// real `Deployed`.
async fn populate(harness: &SandboxHarness, registry_id: &AccountId) -> Result<Populated> {
    let deployer = harness.registry_signer_account_id.clone();
    let market_wasm = templar_gateway_testing::wasm::market().await.to_vec();

    // Every version on every live registry is Normal-mode, which is also the only mode 0.1.0 and
    // 1.0.0 have — and the mode whose stored blob the migration has to rewrite. Only the
    // deployable one needs to be a real contract; the other exists to be soft-deleted, and
    // `remove_version` discards its blob regardless.
    for (key, code) in [
        (MARKET_VERSION, market_wasm),
        (REMOVED_VERSION, b"not a contract".to_vec()),
    ] {
        harness
            .registry_add_version(
                &deployer,
                registry_id,
                key,
                DeployMode::Normal,
                code,
                NearToken::from_yoctonear(1),
            )
            .await?;
    }
    harness
        .call_function_payable(
            &deployer,
            registry_id,
            "remove_version",
            serde_json::json!({ "version_key": REMOVED_VERSION }),
            NearToken::from_yoctonear(1),
        )
        .await?;

    let oracle = harness.create_user("oracle").await?;
    let borrow = harness.create_user("borrow").await?;
    let collateral = harness.create_user("collateral").await?;
    let protocol = harness.create_user("protocol").await?;
    let configuration = test_utils::market_configuration(
        oracle.0,
        borrow.0,
        collateral.0,
        protocol.0,
        YieldWeights::new_with_supply_weight(1),
    );
    harness
        .registry_deploy(
            &deployer,
            registry_id,
            "market",
            MARKET_VERSION,
            serde_json::to_vec(&serde_json::json!({ "configuration": configuration }))?,
            None,
            NearToken::from_near(10),
        )
        .await?;

    let code_hash = harness
        .view_json::<Option<String>>(
            registry_id,
            "get_version_code_hash",
            serde_json::json!({ "version_key": MARKET_VERSION }),
        )
        .await?
        .context("the registered version has a code hash")?;

    Ok(Populated {
        market_id: format!("market.{registry_id}").parse()?,
        code_hash,
    })
}

#[rstest]
#[case::alpha_near("0.1.0")]
#[case::v1_tmplr_near("1.0.0")]
#[case::user0_tmplr_near("1.1.0")]
#[tokio::test]
async fn live_release_migrates_onto_current(
    #[future(awt)] harness: SandboxHarness,
    #[case] release: &str,
) -> Result<()> {
    let registry_id = harness.deploy_registry_version(release).await?;
    let before = populate(&harness, &registry_id).await?;

    let versions_before = harness
        .view_json::<Vec<String>>(&registry_id, "list_versions", serde_json::json!({}))
        .await?;
    let deployments_before = harness
        .view_json::<Vec<AccountId>>(&registry_id, "list_deployments", serde_json::json!({}))
        .await?;
    assert_eq!(deployments_before, vec![before.market_id.clone()]);

    // None of these releases has `upgrade`, so the first migration has to be the batch an
    // operator signs with a full-access key. `upgrade` only takes over afterwards — which is the
    // order the rollout has to follow too.
    harness
        .redeploy_registry_with_migrate(
            templar_gateway_testing::wasm::registry().await.to_vec(),
            migration_for(release),
            near_api::types::NearGas::from_tgas(100),
        )
        .await?;

    assert!(
        !harness
            .view_json::<bool>(&registry_id, "needs_migration", serde_json::json!({}))
            .await?,
        "{release} still reports an outstanding migration",
    );
    assert_eq!(
        harness
            .view_json::<u32>(
                &registry_id,
                "get_stored_state_version",
                serde_json::json!({})
            )
            .await?,
        1,
    );

    // Nothing the registry was holding may have moved.
    assert_eq!(
        harness
            .view_json::<Vec<String>>(&registry_id, "list_versions", serde_json::json!({}))
            .await?,
        versions_before,
    );
    assert_eq!(
        harness
            .view_json::<Vec<AccountId>>(&registry_id, "list_deployments", serde_json::json!({}))
            .await?,
        deployments_before,
    );
    assert_eq!(
        harness
            .view_json::<Option<String>>(
                &registry_id,
                "get_version_code_hash",
                serde_json::json!({ "version_key": MARKET_VERSION }),
            )
            .await?,
        Some(before.code_hash.clone()),
        "the rewritten entry must keep its hash",
    );

    // And the soft-delete must survive as one rather than becoming deployable again.
    let removed = harness
        .view_json::<Option<templar_common::registry::VersionInfo>>(
            &registry_id,
            "get_version",
            serde_json::json!({ "version_key": REMOVED_VERSION }),
        )
        .await?
        .context("the soft-deleted version is still registered")?;
    assert_eq!(removed.availability, VersionAvailability::Removed);

    Ok(())
}

/// Once a registry is on this release, `upgrade` replaces the full-access-key batch — which is
/// what has to be true before the keys can be deleted.
///
/// Upgrades from a global contract rather than an inline blob. Passing the code by hash is the
/// cheaper of the two on any network, and the only one that fits here: a 237 KB blob costs more
/// gas to hand over and decode than is left after reserving [`GAS_FOR_MIGRATE`], under the 300
/// Tgas the sandbox's protocol version allows a transaction.
///
/// [`GAS_FOR_MIGRATE`]: templar_registry_contract::Contract::GAS_FOR_MIGRATE
#[rstest]
#[tokio::test]
async fn upgrade_replaces_the_key_signed_batch(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let registry_id = harness.deploy_registry_version("1.1.0").await?;
    let before = populate(&harness, &registry_id).await?;
    let current = templar_gateway_testing::wasm::registry().await.to_vec();
    harness
        .redeploy_registry_with_migrate(
            current.clone(),
            migration_for("1.1.0"),
            near_api::types::NearGas::from_tgas(100),
        )
        .await?;

    // Publish the new code as a global contract, then upgrade onto it by hash.
    let cost_per_byte = NearToken::from_near(1).saturating_div(10_000);
    harness
        .registry_add_version(
            &harness.registry_signer_account_id.clone(),
            &registry_id,
            SELF_VERSION,
            DeployMode::GlobalHash,
            current.clone(),
            cost_per_byte.saturating_mul(current.len() as u128),
        )
        .await?;
    let global_hash = harness
        .view_json::<Option<String>>(
            &registry_id,
            "get_version_code_hash",
            serde_json::json!({ "version_key": SELF_VERSION }),
        )
        .await?
        .context("the global version has a code hash")?;

    // Already at the target version, so this upgrade carries no migration.
    let result = harness
        .call_function_payable(
            &harness.registry_signer_account_id.clone(),
            &registry_id,
            "upgrade",
            serde_json::json!({
                "code": { "GlobalHash": global_hash },
                "migrate_args": near_sdk::json_types::Base64VecU8(Vec::new()),
            }),
            NearToken::from_yoctonear(1),
        )
        .await?;

    println!(
        "registry upgrade burnt {} Tgas",
        harness.operation_gas_burnt(&result) / 1_000_000_000_000,
    );

    assert_eq!(
        harness
            .view_json::<Vec<AccountId>>(&registry_id, "list_deployments", serde_json::json!({}))
            .await?,
        vec![before.market_id],
    );

    Ok(())
}

#[rstest]
#[tokio::test]
async fn upgrade_rejects_a_non_owner(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let registry_id = harness.deploy_registry().await?;
    let stranger = harness.create_user("stranger").await?;

    let result = harness
        .call_function_payable(
            &stranger,
            &registry_id,
            "upgrade",
            serde_json::json!({
                "code": near_sdk::json_types::Base64VecU8(
                    templar_gateway_testing::wasm::registry().await.to_vec(),
                ),
                "migrate_args": near_sdk::json_types::Base64VecU8(Vec::new()),
            }),
            NearToken::from_yoctonear(1),
        )
        .await;

    assert!(
        result.is_err(),
        "a non-owner upgraded the registry: {result:?}",
    );

    Ok(())
}

/// The largest state one `migrate` can carry — a proof-size ceiling, not a gas one.
///
/// A receipt may record at most 4 MB of trie storage proof
/// (`per_receipt_storage_proof_size_limit`, the same on mainnet and in the sandbox), and the
/// pre-1.1.0 transform reads and rewrites every stored blob. Measured either side of the line:
/// 3.4 MB of blobs migrates, 3.9 MB does not, and both burn about 95 Tgas — so gas is never what
/// runs out, and raising it does not help.
///
/// `v1.tmplr.near` and `templar-alpha.near` each hold roughly 5 MB, so both must be pruned with
/// `remove_version` before they can be migrated at all. This pins the headroom that pruning aims
/// at: if the transform grows costlier per entry, it fails here rather than on the account.
#[rstest]
#[tokio::test]
async fn migrates_the_largest_state_a_receipt_can_carry(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    const VERSIONS: usize = 8;
    const BLOB: usize = 440 * 1024;

    let registry_id = harness.deploy_registry_version("1.0.0").await?;
    let deployer = harness.registry_signer_account_id.clone();
    for index in 0..VERSIONS {
        harness
            .registry_add_version(
                &deployer,
                &registry_id,
                &format!("bulk@{index}"),
                DeployMode::Normal,
                vec![u8::try_from(index % 256)?; BLOB],
                NearToken::from_yoctonear(1),
            )
            .await?;
    }

    let burnt = harness
        .redeploy_registry_with_migrate(
            templar_gateway_testing::wasm::registry().await.to_vec(),
            migration_for("1.0.0"),
            near_api::types::NearGas::from_tgas(280),
        )
        .await?;

    println!(
        "migrated {} MB of blobs for {} Tgas",
        VERSIONS * BLOB / (1024 * 1024),
        burnt.as_gas() / 1_000_000_000_000,
    );

    assert!(
        !harness
            .view_json::<bool>(&registry_id, "needs_migration", serde_json::json!({}))
            .await?,
        "the migration did not complete",
    );
    assert_eq!(
        harness
            .view_json::<Vec<String>>(&registry_id, "list_versions", serde_json::json!({}))
            .await?
            .len(),
        VERSIONS,
        "every version must survive the rewrite",
    );

    Ok(())
}

/// The whole point of `upgrade`: a registry with no access keys left can still be replaced.
///
/// Ownership has to move off the registry first. `new` makes the registry its own owner, so
/// revoking its keys without handing ownership to another account would strand `upgrade` behind a
/// signer that no longer exists — the exact brick this method is meant to prevent. That ordering
/// is the runbook: publish the code, hand over ownership, revoke, then upgrade.
#[rstest]
#[tokio::test]
async fn a_keyless_registry_still_upgrades_through_its_owner(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let registry_id = harness.deploy_registry_version("1.1.0").await?;
    let before = populate(&harness, &registry_id).await?;
    let current = templar_gateway_testing::wasm::registry().await.to_vec();
    let registry_signer = harness.registry_signer_account_id.clone();
    harness
        .redeploy_registry_with_migrate(
            current.clone(),
            migration_for("1.1.0"),
            near_api::types::NearGas::from_tgas(100),
        )
        .await?;

    // Publish the replacement while the registry still owns itself.
    let cost_per_byte = NearToken::from_near(1).saturating_div(10_000);
    harness
        .registry_add_version(
            &registry_signer,
            &registry_id,
            SELF_VERSION,
            DeployMode::GlobalHash,
            current.clone(),
            cost_per_byte.saturating_mul(current.len() as u128),
        )
        .await?;
    let global_hash = harness
        .view_json::<Option<String>>(
            &registry_id,
            "get_version_code_hash",
            serde_json::json!({ "version_key": SELF_VERSION }),
        )
        .await?
        .context("the global version has a code hash")?;

    let new_owner = harness.create_user("registry-owner").await?;
    harness
        .transfer_ownership(&registry_id, &registry_signer, &new_owner)
        .await?;

    harness.revoke_all_access_keys(&registry_id).await?;
    assert!(harness.view_access_keys(&registry_id).await?.is_empty());

    harness
        .call_function_payable(
            &new_owner,
            &registry_id,
            "upgrade",
            serde_json::json!({
                "code": { "GlobalHash": global_hash },
                "migrate_args": near_sdk::json_types::Base64VecU8(Vec::new()),
            }),
            NearToken::from_yoctonear(1),
        )
        .await?;

    // The upgrade landed and took nothing with it.
    assert_eq!(
        harness
            .view_json::<Vec<AccountId>>(&registry_id, "list_deployments", serde_json::json!({}))
            .await?,
        vec![before.market_id],
    );
    assert!(
        !harness
            .view_json::<bool>(&registry_id, "needs_migration", serde_json::json!({}))
            .await?,
    );

    Ok(())
}

/// Naming the wrong migration must cost nothing. Both layouts report a stored version of 0, so
/// the operator picks — and a wrong pick has to leave the registry exactly as it was, still on its
/// old code, rather than half-converted or bricked.
///
/// The revert is what makes the choice safe, and it comes from the deploy and the `migrate`
/// sharing one receipt: the failed migration takes the code with it.
#[rstest]
#[tokio::test]
async fn a_mismatched_migration_leaves_the_registry_untouched(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    // 1.0.0 stores the two-field layout, so the three-field migration cannot read it.
    let registry_id = harness.deploy_registry_version("1.0.0").await?;
    let before = populate(&harness, &registry_id).await?;
    let versions_before = harness
        .view_json::<Vec<String>>(&registry_id, "list_versions", serde_json::json!({}))
        .await?;

    let result = harness
        .redeploy_registry_with_migrate(
            templar_gateway_testing::wasm::registry().await.to_vec(),
            migration_for("1.1.0"),
            near_api::types::NearGas::from_tgas(100),
        )
        .await;
    assert!(
        result.is_err(),
        "a mismatched migration was accepted: {result:?}",
    );

    // Still the old binary: these views only exist on the code that was never deployed.
    assert!(
        harness
            .view_json::<u32>(
                &registry_id,
                "get_stored_state_version",
                serde_json::json!({})
            )
            .await
            .is_err(),
        "the new code survived a failed migration",
    );
    // And still the old state, readable through the API the old binary does serve.
    assert_eq!(
        harness
            .view_json::<Vec<String>>(&registry_id, "list_versions", serde_json::json!({}))
            .await?,
        versions_before,
    );
    assert_eq!(
        harness
            .view_json::<Vec<AccountId>>(&registry_id, "list_deployments", serde_json::json!({}))
            .await?,
        vec![before.market_id],
    );

    Ok(())
}
