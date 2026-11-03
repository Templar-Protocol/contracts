//! Deploy markets from a registry version and verify the deployed configuration
//! and access keys.

use anyhow::{Context, Result};
use near_api::types::AccountId;
use near_sdk::json_types::Base58CryptoHash;
use near_token::NearToken;
use rstest::rstest;
use templar_common::{
    market::MarketConfiguration,
    market::YieldWeights,
    registry::{ApprovedCode, Deployment, VersionAvailability, VersionSource},
};
use templar_gateway_methods_spec::{registry, tx};
use templar_gateway_testing::{harness, publish_deposit_for, SandboxHarness};
use templar_gateway_types::{
    common::{ContractArgs, WriteOperationResult},
    operation::ReceiptStatus,
    primitive::PublicKey,
    Base64Bytes, ContractMethodName, ManagedAccountId,
};

const MARKET_VERSION: &str = "market@0.0.0";
/// A second key for the *same* code, registered by hash instead of by publishing it again.
const BY_HASH_VERSION: &str = "market@0.0.0-by-hash";
// A valid ed25519 public key (the sandbox genesis key) for the access-key test.
const TEST_PUBLIC_KEY: &str = "ed25519:5BGSaf6YjVm7565VzWQHNxoyEjwr3jUpRJSGjREvU9dB";

/// Registering an already-published global stakes nothing: the probe account is created and
/// deleted in one receipt, so it never has to meet a storage minimum.
const PROBE_DEPOSIT: NearToken = NearToken::from_yoctonear(1);

struct Registry {
    id: AccountId,
    deployer: ManagedAccountId,
    configuration: MarketConfiguration,
}

/// Deploy a registry, register the market wasm as a version, and build the
/// market configuration the deploy tests use.
async fn setup_registry(harness: &SandboxHarness) -> Result<Registry> {
    let registry_id = harness.deploy_registry().await?;
    let deployer = harness.registry_signer_account_id.clone();

    let market_wasm = templar_gateway_testing::wasm::market().await.to_vec();
    let deposit = publish_deposit_for(market_wasm.len());
    harness
        .registry_add_version(
            &deployer,
            &registry_id,
            MARKET_VERSION,
            VersionSource::PublishGlobal(market_wasm.into()),
            deposit,
        )
        .await?;

    let configuration = market_configuration(harness).await?;

    Ok(Registry {
        id: registry_id,
        deployer,
        configuration,
    })
}

async fn market_configuration(harness: &SandboxHarness) -> Result<MarketConfiguration> {
    Ok(test_utils::market_configuration(
        harness.create_user("oracle").await?.0,
        harness.create_user("borrow").await?.0,
        harness.create_user("collateral").await?.0,
        harness.create_user("protocol").await?.0,
        YieldWeights::new_with_supply_weight(1),
    ))
}

fn init_args(configuration: &MarketConfiguration) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(
        &serde_json::json!({ "configuration": configuration }),
    )?)
}

#[rstest]
#[tokio::test]
async fn deploy_from_registry(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let registry = setup_registry(&harness).await?;
    let args = init_args(&registry.configuration)?;

    // Deploy the three markets concurrently — they share nothing but the
    // registry, and doing them in sequence needlessly serializes the test.
    let harness = &harness;
    let registry = &registry;
    let deploy = |name: &'static str| {
        let args = args.clone();
        async move {
            harness
                .registry_deploy_without_abi_check(
                    &registry.deployer,
                    &registry.id,
                    name,
                    MARKET_VERSION,
                    args,
                    None,
                    NearToken::from_near(10),
                )
                .await?;

            let market_id: AccountId = format!("{name}.{}", registry.id).parse()?;
            assert_eq!(
                harness.get_configuration(&market_id).await?,
                registry.configuration,
            );
            // Deploying without keys leaves the market with no full-access keys.
            assert!(harness.view_access_keys(&market_id).await?.is_empty());
            Ok::<(), anyhow::Error>(())
        }
    };
    tokio::try_join!(deploy("one"), deploy("two"), deploy("three"))?;

    Ok(())
}

#[rstest]
#[tokio::test]
async fn deploy_with_access_key(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let registry = setup_registry(&harness).await?;
    let key = PublicKey::from(TEST_PUBLIC_KEY.parse::<near_api::types::PublicKey>()?);

    let result = harness
        .registry_deploy_without_abi_check(
            &registry.deployer,
            &registry.id,
            "market",
            MARKET_VERSION,
            init_args(&registry.configuration)?,
            Some(vec![key]),
            NearToken::from_near(10),
        )
        .await?;

    let market_id: AccountId = format!("market.{}", registry.id).parse()?;
    assert_eq!(
        harness.get_configuration(&market_id).await?,
        registry.configuration,
        "the market should deploy with a full-access key requested",
    );

    let deployed: Vec<Deployed> = registry_events(&result, &registry.id, "deployed")?;
    let [deployed] = deployed.as_slice() else {
        panic!("expected one deployed event, got {}", deployed.len());
    };
    assert_eq!(deployed.account_id, market_id);
    assert_eq!(deployed.deployment.version_key, MARKET_VERSION);
    assert_eq!(
        Some(deployed.deployment.code_hash),
        version_code_hash(&harness, &registry.id, MARKET_VERSION).await?,
    );

    // The deployed market must carry exactly the requested full-access key.
    let keys = harness.view_access_keys(&market_id).await?;
    assert_eq!(keys.len(), 1, "expected exactly the one requested key");
    assert_eq!(keys[0].0, TEST_PUBLIC_KEY);
    assert!(keys[0].1, "the requested key must be full-access");

    Ok(())
}

/// Read back the code hash the registry recorded for a version.
async fn version_code_hash(
    harness: &SandboxHarness,
    registry_id: &AccountId,
    version_key: &str,
) -> Result<Option<Base58CryptoHash>> {
    let hash = harness
        .view_json::<Option<String>>(
            registry_id,
            "get_version_code_hash",
            serde_json::json!({ "version_key": version_key }),
        )
        .await?;

    Ok(hash.map(|hash| hash.parse()).transpose()?)
}

/// The point of ENG-631: a second version key pointed at the *same* global contract deploys
/// identically to the one that paid to publish it, for a deposit that is not in the same league.
#[rstest]
#[tokio::test]
async fn deploy_from_a_version_registered_by_code_hash(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let registry = setup_registry(&harness).await?;
    let hash = version_code_hash(&harness, &registry.id, MARKET_VERSION)
        .await?
        .expect("the published version has a code hash");

    let result = harness
        .registry_add_version(
            &registry.deployer,
            &registry.id,
            BY_HASH_VERSION,
            VersionSource::ExistingGlobal(hash),
            PROBE_DEPOSIT,
        )
        .await?;

    let added: Vec<VersionAdded> = registry_events(&result, &registry.id, "version_added")?;
    let [added] = added.as_slice() else {
        panic!("expected one version_added event, got {}", added.len());
    };
    assert_eq!(added.version_key, BY_HASH_VERSION);
    assert_eq!(added.code_hash, hash);
    assert_eq!(added.availability, VersionAvailability::Global);

    // Both keys resolve to one global contract — no second copy of the code was staked.
    assert_eq!(
        version_code_hash(&harness, &registry.id, BY_HASH_VERSION).await?,
        Some(hash),
    );

    harness
        .registry_deploy_without_abi_check(
            &registry.deployer,
            &registry.id,
            "by-hash",
            BY_HASH_VERSION,
            init_args(&registry.configuration)?,
            None,
            NearToken::from_near(10),
        )
        .await?;

    let market_id: AccountId = format!("by-hash.{}", registry.id).parse()?;
    assert_eq!(
        harness.get_configuration(&market_id).await?,
        registry.configuration,
        "a market deployed by code hash must be indistinguishable from a published one",
    );

    // The publish deposit in `setup_registry` stakes storage for every byte of the market wasm;
    // registering the same code by hash stakes nothing, so the two are not remotely comparable.
    let publish_deposit = publish_deposit_for(templar_gateway_testing::wasm::market().await.len());
    assert!(
        publish_deposit.as_yoctonear() / PROBE_DEPOSIT.as_yoctonear() > 1_000_000,
        "registering by hash ({PROBE_DEPOSIT}) must be orders of magnitude below \
         publishing ({publish_deposit})",
    );

    Ok(())
}

/// An unverified hash would burn the version key permanently, because `remove_version` panics on a
/// `GlobalHash` entry. The probe receipt is what makes that state unreachable.
#[rstest]
#[tokio::test]
async fn an_unknown_code_hash_rolls_back_and_frees_the_key(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let registry = setup_registry(&harness).await?;
    let unknown = Base58CryptoHash::from([0xab; 32]);

    let result = harness
        .registry_add_version(
            &registry.deployer,
            &registry.id,
            BY_HASH_VERSION,
            VersionSource::ExistingGlobal(unknown),
            PROBE_DEPOSIT,
        )
        .await;
    assert!(
        result.is_err(),
        "a hash with no global contract behind it must fail the receipt, got: {result:?}",
    );

    assert_eq!(
        version_code_hash(&harness, &registry.id, BY_HASH_VERSION).await?,
        None,
        "the rolled-back key must not survive as an undeployable entry",
    );

    // Reusable: the failed attempt did not consume the name.
    let real = version_code_hash(&harness, &registry.id, MARKET_VERSION)
        .await?
        .expect("the published version has a code hash");
    harness
        .registry_add_version(
            &registry.deployer,
            &registry.id,
            BY_HASH_VERSION,
            VersionSource::ExistingGlobal(real),
            PROBE_DEPOSIT,
        )
        .await?;
    assert_eq!(
        version_code_hash(&harness, &registry.id, BY_HASH_VERSION).await?,
        Some(real),
    );

    Ok(())
}

#[derive(serde::Deserialize)]
struct EventLog {
    standard: String,
    event: String,
    /// Decoded only once the standard and event say what it holds.
    data: serde_json::Value,
}

/// The `templar-registry` events named `event` that the registry's successful receipts logged
/// during the operation's final transaction.
fn registry_events<T: serde::de::DeserializeOwned>(
    result: &WriteOperationResult,
    registry_id: &AccountId,
    event: &str,
) -> Result<Vec<T>> {
    let logs = result
        .operation
        .final_outcome()
        .context("the operation has an outcome")?
        .receipts
        .iter()
        .filter(|receipt| {
            &receipt.contract_id == registry_id && receipt.status == ReceiptStatus::Succeeded
        })
        .flat_map(|receipt| &receipt.logs)
        .filter_map(|log| log.strip_prefix("EVENT_JSON:"))
        .map(serde_json::from_str::<EventLog>)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(logs
        .into_iter()
        .filter(|log| log.standard == "templar-registry" && log.event == event)
        .map(|log| serde_json::from_value(log.data))
        .collect::<Result<_, _>>()?)
}

#[derive(serde::Deserialize)]
struct VersionAdded {
    version_key: String,
    code_hash: Base58CryptoHash,
    availability: VersionAvailability,
}

#[derive(serde::Deserialize)]
struct Deployed {
    account_id: AccountId,
    #[serde(flatten)]
    deployment: Deployment,
}

#[rstest]
#[tokio::test]
async fn a_taken_name_collides(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let registry = setup_registry(&harness).await?;
    let args = init_args(&registry.configuration)?;
    let deploy = || registry::Deploy {
        target: registry::DeployTarget {
            registry_id: registry.id.clone(),
            name: "market".to_owned(),
            version_key: MARKET_VERSION.to_owned(),
            skip_abi_check: true,
            full_access_keys: None,
            deposit: NearToken::from_near(10),
        },
        init_args: Base64Bytes(args.clone()),
    };

    harness.execute(&registry.deployer, deploy()).await?;
    let result = harness.try_execute(&registry.deployer, deploy()).await?;

    let failure = result.operation.failure_message().unwrap_or_default();
    assert!(
        failure.contains("Market ID collision"),
        "expected the collision panic, got: {failure:?}",
    );

    Ok(())
}

/// Upload through the actual Borsh wrapper and recover the contract-computed hash from its event.
async fn upload(
    harness: &SandboxHarness,
    signer: &ManagedAccountId,
    registry_id: &AccountId,
    code: &[u8],
    deposit: NearToken,
) -> Result<Base58CryptoHash> {
    let result = harness
        .call_function_borsh(signer, registry_id, "upload_code", code, deposit)
        .await?;
    let events: Vec<serde_json::Value> = registry_events(&result, registry_id, "code_uploaded")?;
    assert_eq!(events.len(), 1);
    Ok(serde_json::from_value(events[0]["code_hash"].clone())?)
}

async fn upload_info(
    harness: &SandboxHarness,
    registry_id: &AccountId,
    hash: Base58CryptoHash,
) -> Result<Option<templar_common::registry::UploadInfo>> {
    harness
        .view_json(
            registry_id,
            "get_upload",
            serde_json::json!({"code_hash": hash}),
        )
        .await
}

async fn approve(
    harness: &SandboxHarness,
    registry_id: &AccountId,
    key: &str,
    code: ApprovedCode,
) -> Result<WriteOperationResult> {
    harness
        .call_function_payable(
            &harness.registry_signer_account_id,
            registry_id,
            "approve_version",
            serde_json::json!({"version_key": key, "code": code}),
            PROBE_DEPOSIT,
        )
        .await
}

async fn separate_registry(harness: &SandboxHarness) -> Result<AccountId> {
    let account = harness.create_user("approval-registry").await?;
    harness
        .deploy_code(
            &account.0,
            templar_gateway_testing::wasm::registry().await.to_vec(),
        )
        .await?;
    harness
        .call_function(
            &harness.registry_signer_account_id,
            &account.0,
            "new",
            serde_json::json!({}),
        )
        .await?;
    Ok(account.0)
}

#[rstest]
#[tokio::test]
async fn underfunded_upload_rolls_back(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let id = harness.deploy_registry().await?;
    let signer = &harness.registry_signer_account_id;
    let error = harness
        .call_function_borsh(
            signer,
            &id,
            "upload_code",
            vec![0_u8, 1, 2, 255],
            NearToken::ZERO,
        )
        .await
        .expect_err("the actual receipt must reject insufficient storage payment");
    assert!(format!("{error:#}").contains("storage"));
    let uploads: Vec<Base58CryptoHash> = harness
        .view_json(&id, "list_uploads", serde_json::json!({}))
        .await?;
    assert!(
        uploads.is_empty(),
        "receipt rollback must remove all upload metadata"
    );
    let hash = upload(
        &harness,
        signer,
        &id,
        &[0, 1, 2, 255],
        NearToken::from_near(1),
    )
    .await?;
    let info = upload_info(&harness, &id, hash).await?.unwrap();
    assert_eq!(info.uploader, signer.0);
    assert_eq!(info.code_len, 4);
    Ok(())
}

#[rstest]
#[case("Stored")]
#[case("PublishGlobal")]
#[case("ExistingGlobal")]
#[case("Global")]
#[tokio::test]
async fn successful_registration_consumes_matching_market_upload(
    #[future(awt)] harness: SandboxHarness,
    #[case] path: &str,
) -> Result<()> {
    // Hash-only paths require publication elsewhere; publishing twice would exhaust the
    // fixture owner's balance before reaching the callback under test.
    if matches!(path, "ExistingGlobal" | "Global") {
        setup_registry(&harness).await?;
    }
    let id = separate_registry(&harness).await?;
    let code = templar_gateway_testing::wasm::market().await.to_vec();
    let hash = upload(
        &harness,
        &harness.registry_signer_account_id,
        &id,
        &code,
        NearToken::from_near(30),
    )
    .await?;
    assert_eq!(
        hash,
        Base58CryptoHash::from(near_api::types::CryptoHash::hash(&code).0)
    );
    let result = if path == "Global" {
        approve(&harness, &id, "approved", ApprovedCode::Global(hash)).await?
    } else {
        let source = match path {
            "Stored" => VersionSource::Stored(code.into()),
            "PublishGlobal" => VersionSource::PublishGlobal(code.into()),
            "ExistingGlobal" => VersionSource::ExistingGlobal(hash),
            _ => unreachable!(),
        };
        let deposit = if path == "PublishGlobal" {
            publish_deposit_for(templar_gateway_testing::wasm::market().await.len())
        } else {
            PROBE_DEPOSIT
        };
        harness
            .registry_add_version(
                &harness.registry_signer_account_id,
                &id,
                "approved",
                source,
                deposit,
            )
            .await?
    };
    assert!(upload_info(&harness, &id, hash).await?.is_none());
    assert_eq!(
        Some(hash),
        version_code_hash(&harness, &id, "approved").await?
    );
    assert_eq!(
        registry_events::<VersionAdded>(&result, &id, "version_added")?.len(),
        1
    );
    assert!(registry_events::<serde_json::Value>(&result, &id, "upload_removed")?.is_empty());
    // In particular, the successful global callback deletes a full market-sized upload in its
    // production gas allocation, rather than proving cleanup with a tiny synthetic blob.
    assert!(upload(
        &harness,
        &harness.registry_signer_account_id,
        &id,
        templar_gateway_testing::wasm::market().await,
        NearToken::from_near(30),
    )
    .await
    .is_err());
    Ok(())
}

/// Read stored versions through the public chunk API, checking the complete byte sequence.
async fn assert_version_bytes(
    harness: &SandboxHarness,
    registry_id: &AccountId,
    key: &str,
    expected: &[u8],
) -> Result<()> {
    let mut restored = Vec::new();
    while restored.len() < expected.len() {
        let chunk = near_api::Contract(registry_id.clone())
            .call_function(
                "get_version_code_chunk",
                serde_json::json!({
                    "version_key": key, "offset": restored.len(), "len": 16_384,
                }),
            )
            .read_only_borsh::<Option<Vec<u8>>>()
            .fetch_from(&harness.network)
            .await?
            .data
            .context("approved bytes must remain available")?;
        assert!(!chunk.is_empty());
        restored.extend(chunk);
    }
    assert_eq!(restored, expected);
    Ok(())
}

#[rstest]
#[case("Global")]
#[case("Uploaded")]
#[case("Remove")]
#[tokio::test]
async fn non_tail_upload_cleanup_preserves_unrelated_blob(
    #[future(awt)] harness: SandboxHarness,
    #[case] path: &str,
) -> Result<()> {
    let signer = &harness.registry_signer_account_id;
    let head = templar_gateway_testing::wasm::mock_dao().await;
    if path == "Global" {
        let source = harness.deploy_registry().await?;
        harness
            .registry_add_version(
                signer,
                &source,
                "published",
                VersionSource::PublishGlobal(head.to_vec().into()),
                publish_deposit_for(head.len()),
            )
            .await?;
    }
    let id = separate_registry(&harness).await?;
    let head_hash = upload(&harness, signer, &id, head, NearToken::from_near(3)).await?;
    // Distinct, nonuniform arbitrary bytes make byte-for-byte preservation meaningful.
    let tail: Vec<u8> = (0_u8..251).cycle().take(1_048_576).collect();
    let tail_hash = upload(&harness, signer, &id, &tail, NearToken::from_near(15)).await?;
    let tail_before = upload_info(&harness, &id, tail_hash).await?.unwrap();

    let result = if path == "Remove" {
        harness
            .call_function_payable(
                signer,
                &id,
                "remove_upload",
                serde_json::json!({"code_hash": head_hash}),
                PROBE_DEPOSIT,
            )
            .await?
    } else {
        let code = if path == "Global" {
            ApprovedCode::Global(head_hash)
        } else {
            ApprovedCode::Uploaded(head_hash)
        };
        // Match the DAO action's budget, not the harness's default 300 Tgas.
        harness
            .try_execute(
                signer,
                tx::FunctionCall {
                    receiver_id: id.clone(),
                    method_name: ContractMethodName("approve_version".to_owned()),
                    args: ContractArgs::Json(serde_json::json!({
                        "version_key": "head", "code": code,
                    })),
                    gas: near_api::types::NearGas::from_tgas(80),
                    deposit: PROBE_DEPOSIT,
                },
            )
            .await?
    };
    assert!(templar_gateway_testing::failed_receipts(&result)
        .next()
        .is_none());
    let added = registry_events::<VersionAdded>(&result, &id, "version_added")?;
    if path == "Remove" {
        assert!(added.is_empty());
        let removed = registry_events::<serde_json::Value>(&result, &id, "upload_removed")?;
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0]["code_hash"], serde_json::to_value(head_hash)?);
        // No refund promise: explicit removal executes solely on the registry.
        assert!(result
            .operation
            .final_outcome()
            .context("removal has an outcome")?
            .receipts
            .iter()
            .all(|receipt| receipt.contract_id == id));
    } else {
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].version_key, "head");
        assert_eq!(added[0].code_hash, head_hash);
        assert_eq!(
            version_code_hash(&harness, &id, "head").await?,
            Some(head_hash)
        );
        assert!(registry_events::<serde_json::Value>(&result, &id, "upload_removed")?.is_empty());
        if path == "Uploaded" {
            assert_version_bytes(&harness, &id, "head", head).await?;
        }
    }
    assert!(upload_info(&harness, &id, head_hash).await?.is_none());
    assert_eq!(
        upload_info(&harness, &id, tail_hash).await?.unwrap(),
        tail_before
    );
    let converted = approve(&harness, &id, "tail", ApprovedCode::Uploaded(tail_hash)).await?;
    assert!(templar_gateway_testing::failed_receipts(&converted)
        .next()
        .is_none());
    assert!(upload_info(&harness, &id, tail_hash).await?.is_none());
    assert_version_bytes(&harness, &id, "tail", &tail).await?;
    Ok(())
}

#[rstest]
#[tokio::test]
async fn uploaded_approval_moves_bytes_and_preserves_key(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let configuration = market_configuration(&harness).await?;
    let id = separate_registry(&harness).await?;
    let signer = &harness.registry_signer_account_id;
    let code = templar_gateway_testing::wasm::market().await.to_vec();
    let hash = upload(&harness, signer, &id, &code, NearToken::from_near(30)).await?;
    assert!(approve(
        &harness,
        &id,
        "missing",
        ApprovedCode::Uploaded([0xab; 32].into())
    )
    .await
    .is_err());
    assert!(upload_info(&harness, &id, hash).await?.is_some());
    let result = approve(&harness, &id, "market", ApprovedCode::Uploaded(hash)).await?;
    let added = registry_events::<VersionAdded>(&result, &id, "version_added")?;
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].code_hash, hash);
    assert!(upload_info(&harness, &id, hash).await?.is_none());
    assert_version_bytes(&harness, &id, "market", &code).await?;
    harness
        .registry_deploy_without_abi_check(
            signer,
            &id,
            "market",
            "market",
            init_args(&configuration)?,
            None,
            NearToken::from_near(10),
        )
        .await?;
    let child: AccountId = format!("market.{id}").parse()?;
    assert_eq!(harness.get_configuration(&child).await?, configuration);
    assert_eq!(
        harness
            .code_hash(&child)
            .await?
            .parse::<Base58CryptoHash>()?,
        hash
    );
    let pending = upload(&harness, signer, &id, &[1, 2, 3], NearToken::from_near(1)).await?;
    assert!(
        approve(&harness, &id, "market", ApprovedCode::Uploaded(pending))
            .await
            .is_err()
    );
    assert!(upload_info(&harness, &id, pending).await?.is_some());
    harness
        .call_function_payable(
            signer,
            &id,
            "remove_version",
            serde_json::json!({"version_key": "market"}),
            PROBE_DEPOSIT,
        )
        .await?;
    assert!(
        approve(&harness, &id, "market", ApprovedCode::Uploaded(pending))
            .await
            .is_err()
    );
    assert!(upload_info(&harness, &id, pending).await?.is_some());
    assert!(
        upload(&harness, signer, &id, &code, NearToken::from_near(30))
            .await
            .is_err()
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn failed_global_approval_preserves_upload_and_releases_key(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let id = harness.deploy_registry().await?;
    let signer = &harness.registry_signer_account_id;
    // Arbitrary bytes are valid upload payloads, but this hash has never been published.
    let hash = upload(
        &harness,
        signer,
        &id,
        &[0, 1, 2, 255],
        NearToken::from_near(1),
    )
    .await?;
    let before = upload_info(&harness, &id, hash).await?.unwrap();
    let failed = harness
        .try_execute(
            signer,
            tx::FunctionCall {
                receiver_id: id.clone(),
                method_name: ContractMethodName("approve_version".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "version_key": "retry", "code": {"Global": hash},
                })),
                gas: near_api::types::NearGas::from_tgas(300),
                deposit: PROBE_DEPOSIT,
            },
        )
        .await?;
    assert!(templar_gateway_testing::failed_receipts(&failed)
        .next()
        .is_some());
    assert!(registry_events::<VersionAdded>(&failed, &id, "version_added")?.is_empty());
    assert_eq!(version_code_hash(&harness, &id, "retry").await?, None);
    assert_eq!(upload_info(&harness, &id, hash).await?.unwrap(), before);
    approve(&harness, &id, "retry", ApprovedCode::Uploaded(hash)).await?;
    assert!(upload_info(&harness, &id, hash).await?.is_none());
    assert!(harness
        .registry_deploy_without_abi_check(
            signer,
            &id,
            "invalid",
            "retry",
            b"{}".to_vec(),
            None,
            NearToken::from_near(10),
        )
        .await
        .is_err());
    Ok(())
}

#[rstest]
#[case(Vec::new())]
#[case(vec![0, 1, 2, 255])]
#[tokio::test]
async fn duplicate_upload_does_not_replace_original(
    #[future(awt)] harness: SandboxHarness,
    #[case] bytes: Vec<u8>,
) -> Result<()> {
    let id = harness.deploy_registry().await?;
    let owner = &harness.registry_signer_account_id;
    let other = harness.create_user("other-uploader").await?;
    harness
        .call_function_payable(
            owner,
            &id,
            "grant_role",
            serde_json::json!({"account_id": other.0, "role": "Uploader"}),
            PROBE_DEPOSIT,
        )
        .await?;
    let hash = upload(&harness, owner, &id, &bytes, NearToken::from_near(1)).await?;
    for signer in [owner, &other] {
        assert!(
            upload(&harness, signer, &id, &bytes, NearToken::from_near(1))
                .await
                .is_err()
        );
        assert_eq!(
            upload_info(&harness, &id, hash).await?.unwrap().uploader,
            owner.0
        );
    }
    approve(&harness, &id, "invalid", ApprovedCode::Uploaded(hash)).await?;
    assert!(harness
        .registry_deploy_without_abi_check(
            owner,
            &id,
            "invalid",
            "invalid",
            b"{}".to_vec(),
            None,
            NearToken::from_near(10),
        )
        .await
        .is_err());
    Ok(())
}
