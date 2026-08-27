//! Deploy markets from a registry version and verify the deployed configuration
//! and access keys.

use anyhow::Result;
use near_api::types::AccountId;
use near_sdk::json_types::Base58CryptoHash;
use near_token::NearToken;
use rstest::rstest;
use templar_common::{market::MarketConfiguration, market::YieldWeights, registry::VersionSource};
use templar_gateway_testing::{harness, publish_deposit_for, SandboxHarness};
use templar_gateway_types::{primitive::PublicKey, ManagedAccountId};

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

    // The assets only need to be valid ids in the configuration — the deployed
    // market validates the config's shape, not that these accounts exist.
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

    Ok(Registry {
        id: registry_id,
        deployer,
        configuration,
    })
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

    harness
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

    harness
        .registry_add_version(
            &registry.deployer,
            &registry.id,
            BY_HASH_VERSION,
            VersionSource::ExistingGlobal(hash),
            PROBE_DEPOSIT,
        )
        .await?;

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

#[rstest]
#[tokio::test]
async fn market_id_collision(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let registry = setup_registry(&harness).await?;
    let args = init_args(&registry.configuration)?;

    harness
        .registry_deploy_without_abi_check(
            &registry.deployer,
            &registry.id,
            "market",
            MARKET_VERSION,
            args.clone(),
            None,
            NearToken::from_near(10),
        )
        .await?;
    // Re-deploying the same name collides.
    let result = harness
        .registry_deploy_without_abi_check(
            &registry.deployer,
            &registry.id,
            "market",
            MARKET_VERSION,
            args,
            None,
            NearToken::from_near(10),
        )
        .await;
    assert!(
        result.is_err()
            && format!("{:#}", result.as_ref().unwrap_err()).contains("Market ID collision"),
        "expected a Market ID collision error, got: {result:?}",
    );

    Ok(())
}
