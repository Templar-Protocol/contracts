use anyhow::{Context as _, Result};
use clap::Parser as _;
use near_account_id::AccountId;
use near_primitives::{
    account::{AccessKey, Account as ChainAccount, AccountContract},
    hash::CryptoHash,
    state_record::StateRecord,
};
use near_token::NearToken;
use rstest::rstest;
use templar_common::registry::VersionSource;
use templar_contract_artifacts::{fetch, ArtifactId};
use templar_gateway_client::Client;
use templar_gateway_methods_spec::{contract, owner, registry, tx};
use templar_gateway_testing::SandboxHarness;
use templar_gateway_types::{
    common::{ContractArgs, Pagination},
    Base64Bytes, ContractMethodName, OperationStatus,
};

use super::TEST_SECRET_KEY;
use crate::cli::{Cli, Command};
use crate::commands::registry::RegistryNs;

/// What the registry's `new` takes.
#[derive(serde::Serialize)]
struct NoArgs {}

const LIVE_VERSION: &str = "market@0.0.0";
const REMOVED_VERSION: &str = "market@0.0.0-removed";

const LIVE_RELEASE: &str = "2.0.0";

/// A `.near` account holding only the fixed test key.
async fn keyed_account(harness: &SandboxHarness, account_id: &AccountId) -> Result<()> {
    let public_key: near_crypto::PublicKey = TEST_SECRET_KEY
        .parse::<near_api::SecretKey>()?
        .public_key()
        .to_string()
        .parse()?;
    templar_sandbox::patch_records(
        &harness.network,
        vec![
            StateRecord::Account {
                account_id: account_id.clone(),
                account: ChainAccount::new(
                    NearToken::from_near(1_000),
                    NearToken::from_yoctonear(0),
                    AccountContract::None,
                    182,
                ),
            },
            StateRecord::AccessKey {
                account_id: account_id.clone(),
                public_key: public_key.clone(),
                access_key: AccessKey::full_access(),
            },
        ],
    )
    .await?;
    templar_sandbox::wait_until_final(&harness.network, account_id, &public_key).await?;
    Ok(())
}

/// A `.near` registry running released `release`, owning itself and signed for by the fixed test
/// key — the shape of `templar-alpha.near` and `user0.tmplr.near`.
async fn live_registry(
    harness: &SandboxHarness,
    registry_id: &AccountId,
    release: &str,
) -> Result<Client> {
    keyed_account(harness, registry_id).await?;
    // Final, so the setup is in the finalized state the upgrade snapshots, not still landing.
    let client = Client::builder(harness.network.clone())
        .secret_key(registry_id.clone(), TEST_SECRET_KEY.parse()?)?
        .finality_policy(templar_gateway_core::FinalityPolicy::Final)
        .build()?;
    let deployed = client
        .execute_as(
            registry_id.clone(),
            tx::DeployAndInit {
                account_id: registry_id.clone(),
                code: Base64Bytes(fetch::released_bytes(ArtifactId::Registry, release).await?),
                method_name: ContractMethodName("new".to_owned()),
                args: ContractArgs::Json(serde_json::to_value(NoArgs {})?),
                gas: templar_gateway_types::NearGas::from_tgas(30),
                deposit: NearToken::from_yoctonear(0),
            },
        )
        .await?;
    anyhow::ensure!(deployed.operation.status == OperationStatus::Succeeded);
    Ok(client)
}

async fn add_version(
    client: &Client,
    registry_id: &AccountId,
    version_key: &str,
    code: Vec<u8>,
) -> Result<()> {
    let result = client
        .execute_as(
            registry_id.clone(),
            registry::AddVersion {
                registry_id: registry_id.clone(),
                version_key: version_key.to_owned(),
                source: VersionSource::Stored(code.into()),
                deposit: NearToken::from_yoctonear(1),
            },
        )
        .await?;
    anyhow::ensure!(result.operation.status == OperationStatus::Succeeded);
    Ok(())
}

/// A deployable version, a soft-deleted one and a deployment, so both maps have something the
/// upgrade must carry.
async fn populate(client: &Client, registry_id: &AccountId, release: &str) -> Result<()> {
    let child_code = fetch::released_bytes(ArtifactId::Registry, release).await?;
    add_version(client, registry_id, LIVE_VERSION, child_code).await?;
    add_version(
        client,
        registry_id,
        REMOVED_VERSION,
        b"not a contract".to_vec(),
    )
    .await?;
    let removed = client
        .execute_as(
            registry_id.clone(),
            registry::RemoveVersion {
                registry_id: registry_id.clone(),
                version_key: REMOVED_VERSION.to_owned(),
            },
        )
        .await?;
    anyhow::ensure!(removed.operation.status == OperationStatus::Succeeded);
    let deployed = client
        .execute_as(
            registry_id.clone(),
            registry::Deploy {
                target: registry::DeployTarget {
                    registry_id: registry_id.clone(),
                    name: "child".to_owned(),
                    version_key: LIVE_VERSION.to_owned(),
                    skip_abi_check: true,
                    full_access_keys: None,
                    deposit: NearToken::from_near(5),
                },
                init_args: Base64Bytes(b"{}".to_vec()),
            },
        )
        .await?;
    anyhow::ensure!(deployed.operation.status == OperationStatus::Succeeded);
    Ok(())
}

async fn run_upgrade(
    harness: &SandboxHarness,
    registry_id: &AccountId,
    signer_id: &AccountId,
) -> Result<()> {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "--network",
        "mainnet",
        "--rpc-url",
        harness.network.rpc_endpoints[0].url.as_str(),
        "-q",
        "registry",
        "upgrade",
        "--registry-id",
        registry_id.as_str(),
        "--signer-id",
        signer_id.as_str(),
        "--secret-key",
        TEST_SECRET_KEY,
    ])?;
    let ctx = crate::context::build_context(&cli)?;
    crate::dispatch::dispatch(ctx, cli.command).await
}

async fn released_hash(release: &str) -> Result<String> {
    Ok(
        CryptoHash::hash_bytes(&fetch::released_bytes(ArtifactId::Registry, release).await?)
            .to_string(),
    )
}

/// Hand `registry_id` to `owner_id`, the shape of `v1.tmplr.near` under `tmplr.near`.
async fn hand_over(
    harness: &SandboxHarness,
    client: &Client,
    registry_id: &AccountId,
    owner_id: &AccountId,
) -> Result<Client> {
    keyed_account(harness, owner_id).await?;
    let owner_client = Client::builder(harness.network.clone())
        .secret_key(owner_id.clone(), TEST_SECRET_KEY.parse()?)?
        .finality_policy(templar_gateway_core::FinalityPolicy::Final)
        .build()?;
    let proposed = client
        .execute_as(
            registry_id.clone(),
            owner::ProposeOwner {
                contract_id: registry_id.clone(),
                account_id: Some(owner_id.clone()),
            },
        )
        .await?;
    anyhow::ensure!(proposed.operation.status == OperationStatus::Succeeded);
    let accepted = owner_client
        .execute_as(
            owner_id.clone(),
            owner::AcceptOwner {
                contract_id: registry_id.clone(),
            },
        )
        .await?;
    anyhow::ensure!(accepted.operation.status == OperationStatus::Succeeded);
    Ok(owner_client)
}

#[rstest]
#[case::self_owned(None)]
#[case::owned_by_another_account(Some("owner.near"))]
#[tokio::test]
async fn requires_sandbox_registry_upgrade_carries_a_live_registry(
    #[case] owner: Option<&str>,
) -> Result<()> {
    let harness = SandboxHarness::start_owned().await?;
    let registry_id: AccountId = "upgrade-me.near".parse()?;
    let client = live_registry(&harness, &registry_id, LIVE_RELEASE).await?;
    populate(&client, &registry_id, LIVE_RELEASE).await?;
    let signer_id = match owner {
        Some(owner) => {
            let owner: AccountId = owner.parse()?;
            hand_over(&harness, &client, &registry_id, &owner).await?;
            owner
        }
        None => registry_id.clone(),
    };
    let versions_before = client
        .read(registry::ListVersions {
            registry_id: registry_id.clone(),
            args: Pagination::default(),
        })
        .await?;

    run_upgrade(&harness, &registry_id, &signer_id).await?;
    let client = Client::read_only(harness.network.clone())?;

    let target = ArtifactId::Registry
        .metadata()
        .version()
        .context("a registry release is catalogued")?;
    assert_eq!(
        harness.code_hash(&registry_id).await?,
        released_hash(target).await?
    );
    let state = client
        .read(contract::GetStateVersion {
            contract_id: registry_id.clone(),
        })
        .await?;
    assert!(!state.needs_migration, "{state:?}");
    assert_eq!(
        client
            .read(registry::ListVersions {
                registry_id: registry_id.clone(),
                args: Pagination::default(),
            })
            .await?,
        versions_before,
    );
    let removed = client
        .read(registry::GetVersion {
            registry_id: registry_id.clone(),
            version_key: REMOVED_VERSION.to_owned(),
        })
        .await?
        .context("the soft-deleted version is still registered")?;
    assert_eq!(
        removed.availability,
        templar_common::registry::VersionAvailability::Removed
    );
    assert!(client
        .read(registry::GetDeployment {
            registry_id: registry_id.clone(),
            account_id: format!("child.{registry_id}").parse()?,
        })
        .await?
        .is_some());

    Ok(())
}

/// Only the owner may call `upgrade`, so any other signer is refused before anything is sent.
#[tokio::test]
async fn requires_sandbox_registry_upgrade_refuses_a_signer_that_is_not_the_owner() -> Result<()> {
    let harness = SandboxHarness::start_owned().await?;
    let registry_id: AccountId = "owned.near".parse()?;
    let client = live_registry(&harness, &registry_id, LIVE_RELEASE).await?;
    hand_over(&harness, &client, &registry_id, &"owner.near".parse()?).await?;

    let error = run_upgrade(&harness, &registry_id, &registry_id)
        .await
        .expect_err("a non-owner must not upgrade");

    let message = error.to_string();
    assert!(
        message.contains("(upgrade.signer)") && message.contains("the upgrade was not submitted"),
        "{message}"
    );
    Ok(())
}

#[test]
fn parses_registry_upgrade() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "registry",
        "upgrade",
        "--registry-id",
        "templar-alpha.near",
        "--release",
        "2.0.0",
        "--signer-id",
        "templar-alpha.near",
        "--secret-key",
        TEST_SECRET_KEY,
    ])
    .expect("upgrade should parse");
    let Command::Registry {
        command: RegistryNs::Upgrade(upgrade),
    } = cli.command
    else {
        panic!("expected Registry::Upgrade");
    };
    assert_eq!(upgrade.registry_id().as_str(), "templar-alpha.near");
    assert_eq!(upgrade.release(), Some("2.0.0"));
}
