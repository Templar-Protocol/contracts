//! The upgrade this release actually performs: mainnet's `0.3.0` proxy oracles, on v1 state, take
//! the post-audit code **without a migration**.
//!
//! Every persisted Borsh layout survived the audit remediation, so `StateVersion::VERSION` is still
//! `1` and no migration exists to run. What the remediation did add is validation the new runtime
//! applies to state it did not write — so the claim worth testing is not "the bytes decode" in the
//! abstract, but "the bytes *mainnet actually holds* decode, and still satisfy the rules the new
//! code enforces when it loads them".
//!
//! The fixture is therefore a real storage trie, not a constructed one. Refresh it with
//! `generate_mainnet_upgrade_state_patch`.

#![allow(clippy::unwrap_used)]

mod common;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use near_api::{types::AccountId, Contract, NetworkConfig};
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_methods_spec::proxy_oracle;
use templar_gateway_testing::{ArtifactId, SandboxHarness};

use common::StatePatch;

/// The release every mainnet proxy oracle runs, and so the only version this upgrade starts from.
const DEPLOYED_VERSION: &str = "0.3.0";

/// The account the fixture was taken from.
const FIXTURE_ACCOUNT: &str = "proxy-oracle-ixlmustry-ixlmusdc.v1.tmplr.near";

const USTRY_PRICE_ID: PriceIdentifier =
    PriceIdentifier(*b"USTRY\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
const USDC_PRICE_ID: PriceIdentifier =
    PriceIdentifier(*b"USDC\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");

fn patch_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/migration/mainnet_v1_proxy_oracle_ixlmustry_ixlmusdc.borsh")
}

fn patch() -> StatePatch {
    near_sdk::borsh::from_slice(include_bytes!(
        "./migration/mainnet_v1_proxy_oracle_ixlmustry_ixlmusdc.borsh"
    ))
    .unwrap()
}

/// Reproduce the live deployment — released `0.3.0` code carrying mainnet's own v1 state — then
/// replace only the code, exactly as an upgrade does.
async fn upgrade_from_mainnet_state(harness: &SandboxHarness) -> Result<AccountId> {
    let account_id = harness.proxy_oracle_signer_account_id.0.clone();

    harness
        .deploy_code(
            &account_id,
            templar_gateway_testing::wasm::released(ArtifactId::ProxyOracle, DEPLOYED_VERSION)
                .await,
        )
        .await?;
    harness.patch_state(&account_id, patch()).await?;

    // The upgrade under test: new code over untouched state, and no `migrate` call, because the
    // release defines no migration to run.
    harness
        .deploy_code(
            &account_id,
            templar_gateway_testing::wasm::proxy_oracle().await.to_vec(),
        )
        .await?;

    Ok(account_id)
}

/// Re-dump the fixture from the live mainnet deployment. Raw `near_api` on purpose: this reads an
/// account's whole storage trie rather than calling a contract method, so it has no gateway
/// operation to go through.
#[tokio::test]
#[ignore = "fixture generator"]
async fn generate_mainnet_upgrade_state_patch() -> Result<()> {
    let network = NetworkConfig::mainnet();
    let account_id: AccountId = FIXTURE_ACCOUNT.parse()?;
    let storage = Contract(account_id)
        .view_storage()
        .fetch_from(&network)
        .await?
        .data;
    let state_patch: StatePatch = storage
        .values
        .into_iter()
        .map(|entry| {
            (
                STANDARD.decode(entry.key.0).unwrap(),
                STANDARD.decode(entry.value.0).unwrap(),
            )
        })
        .collect();
    fs::write(patch_path(), near_sdk::borsh::to_vec(&state_patch).unwrap()).unwrap();
    Ok(())
}

/// The fixture must be what this test claims it is: the released code, on v1 state, needing no
/// migration *before* the upgrade. Otherwise the upgrade assertions below prove nothing.
#[tokio::test]
async fn mainnet_upgrade_starts_from_the_released_version() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let account_id = harness.proxy_oracle_signer_account_id.0.clone();

    harness
        .deploy_code(
            &account_id,
            templar_gateway_testing::wasm::released(ArtifactId::ProxyOracle, DEPLOYED_VERSION)
                .await,
        )
        .await?;
    harness.patch_state(&account_id, patch()).await?;

    assert_eq!(
        harness.contract_version(&account_id).await?,
        DEPLOYED_VERSION
    );
    let version = harness.contract_state_version(&account_id).await?;
    assert_eq!(version.stored, 1, "the fixture is v1 state");
    assert!(!version.needs_migration);

    Ok(())
}

/// The release's central claim: new code loads mainnet's stored state untouched.
#[tokio::test]
async fn mainnet_upgrade_needs_no_migration() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let client = harness.client()?;
    let oracle_id = upgrade_from_mainnet_state(&harness).await?;

    // The upgrade really happened, and left the state version where it was.
    assert_ne!(
        harness.contract_version(&oracle_id).await?,
        DEPLOYED_VERSION,
        "the new code should be deployed"
    );
    let version = harness.contract_state_version(&oracle_id).await?;
    assert_eq!(version.stored, 1);
    assert_eq!(
        version.target, 1,
        "the release defines no new state version"
    );
    assert!(
        !version.needs_migration,
        "no migration should be outstanding after the upgrade"
    );

    // Every feed mainnet had is still there, still readable through the new types.
    let mut proxies = client
        .read(proxy_oracle::ListProxies {
            oracle_id: oracle_id.clone(),
            offset: None,
            count: None,
        })
        .await?
        .proxies;
    proxies.sort();
    assert_eq!(proxies, vec![USDC_PRICE_ID, USTRY_PRICE_ID]);

    for price_id in proxies {
        assert!(
            client
                .read(proxy_oracle::GetProxy {
                    oracle_id: oracle_id.clone(),
                    id: price_id,
                })
                .await?
                .proxy
                .is_some(),
            "{price_id} lost its proxy definition across the upgrade"
        );
    }

    Ok(())
}

/// The hazard a preflight exists to catch: the new runtime treats a breaker set it will not accept
/// as *absent*, which silently disarms the feed. Mainnet's stored sets have to survive that gate.
#[tokio::test]
async fn mainnet_upgrade_preserves_loadable_breaker_sets() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let client = harness.client()?;
    let oracle_id = upgrade_from_mainnet_state(&harness).await?;

    for price_id in [USDC_PRICE_ID, USTRY_PRICE_ID] {
        // Deserializing into the post-audit `CircuitBreakerSet` is itself half the assertion: a set
        // the new types cannot parse is exactly the failure mode being ruled out.
        let set = client
            .read(proxy_oracle::GetProxyCircuitBreakerSet {
                oracle_id: oracle_id.clone(),
                id: price_id,
            })
            .await?
            .circuit_breaker_set
            .unwrap_or_else(|| panic!("{price_id} has no stored breaker set"));

        set.validate().unwrap_or_else(|error| {
            panic!("{price_id}'s stored breaker set is inert under the new rules: {error}")
        });
    }

    Ok(())
}

/// The other side of the same gate: a set the *previous* release accepted, which the new one will
/// not, must fail closed rather than quietly evaluate.
///
/// `MonotonicRun` now requires an unsampled history and a streak shorter than the history it reads,
/// neither of which the old validation enforced — so this is a configuration a real deployment
/// could be holding, not an invented one. It is written straight into storage through
/// [`UncheckedCircuitBreakerSet`] because the new contract exists precisely to refuse it.
#[tokio::test]
async fn mainnet_upgrade_disarms_a_breaker_set_the_new_rules_reject() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let client = harness.client()?;
    let account_id = harness.proxy_oracle_signer_account_id.0.clone();

    harness
        .deploy_code(
            &account_id,
            templar_gateway_testing::wasm::released(ArtifactId::ProxyOracle, DEPLOYED_VERSION)
                .await,
        )
        .await?;

    let mut patch = patch();
    overwrite_breaker_set(&mut patch, USTRY_PRICE_ID, inert_breaker_set_bytes());
    harness.patch_state(&account_id, patch).await?;

    harness
        .deploy_code(
            &account_id,
            templar_gateway_testing::wasm::proxy_oracle().await.to_vec(),
        )
        .await?;

    // Reported as absent, not as a set that merely fails to trip: this is what makes an operator
    // think the feed is defended when it is not.
    assert!(
        client
            .read(proxy_oracle::GetProxyCircuitBreakerSet {
                oracle_id: account_id.clone(),
                id: USTRY_PRICE_ID,
            })
            .await?
            .circuit_breaker_set
            .is_none(),
        "an inert set must not read back as usable protection"
    );

    // The untouched feed is unaffected, so the failure is per-asset rather than contract-wide.
    assert!(client
        .read(proxy_oracle::GetProxyCircuitBreakerSet {
            oracle_id: account_id,
            id: USDC_PRICE_ID,
        })
        .await?
        .circuit_breaker_set
        .is_some());

    Ok(())
}

/// Overwrite the breaker set stored for `price_id`.
///
/// `circuit_breakers` is an `UnorderedMap` under prefix `\x01` (`StorageKey::CircuitBreakers`), so a
/// value does not live under its own key: `\x01i<price_id>` holds the entry's index, and the value
/// sits at `\x01v<index>`. Writing to the key itself would leave the real slot untouched.
fn overwrite_breaker_set(patch: &mut StatePatch, price_id: PriceIdentifier, value: Vec<u8>) {
    let mut index_key = b"\x01i".to_vec();
    index_key.extend_from_slice(&price_id.0);
    let index = patch
        .get(&index_key)
        .unwrap_or_else(|| panic!("{price_id} has no breaker-map index in the fixture"));
    let Ok(index) = <[u8; 8]>::try_from(index.as_slice()) else {
        panic!("{price_id}'s breaker-map index is not a u64");
    };

    let mut value_key = b"\x01v".to_vec();
    value_key.extend_from_slice(&index);
    assert!(
        patch.insert(value_key, value).is_some(),
        "{price_id}'s breaker slot should already hold a set"
    );
}

/// A structurally valid set whose only rule can never fire: `MonotonicRun` needs `max_streak` to be
/// shorter than the retained history, and this one is longer.
fn inert_breaker_set_bytes() -> Vec<u8> {
    use std::collections::BTreeMap;
    use templar_common::Decimal;
    use templar_proxy_oracle_kernel::proxy::circuit_breaker::{
        CircuitBreaker, CircuitBreakerState, MonotonicRun, RingBuffer, UncheckedCircuitBreakerSet,
    };

    let inert = UncheckedCircuitBreakerSet {
        sample_interval_ns: templar_common::Nanoseconds::zero(),
        accepted_history: RingBuffer::new(4),
        observed_history: RingBuffer::new(4),
        next_id: 1,
        is_manually_tripped: false,
        breakers: BTreeMap::from([(
            0,
            CircuitBreakerState::new(CircuitBreaker::MonotonicRun(MonotonicRun {
                max_streak: 64,
                min_relative_step_change: Decimal::ONE_HALF,
            })),
        )]),
    };
    near_sdk::borsh::to_vec(&inert).unwrap()
}
