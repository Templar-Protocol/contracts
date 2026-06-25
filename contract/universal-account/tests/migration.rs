#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use common::{
    call, create_account, deploy_code, deploy_with_init, execute_as, get_counter, get_key, harness,
    increment_action, list_keys, migrate, needs_migration, patch_state_version, patch_storage,
    stored_state_version, target_state_version, test_signer, to_sdk, ua_id, view_succeeds,
    CallOutcome,
};
use near_api::{AccountId, Signer};
use near_sdk::{
    borsh,
    json_types::{U128, U64},
    Gas,
};
use near_token::NearToken;
use rstest::rstest;
use templar_gateway_testing::SandboxHarness;
use templar_universal_account::{
    authentication::{with_raw_string::WithRawString, Payload},
    state,
    transaction::Transaction,
    InitArgs, KeyId, NEAR_TESTNET_CHAIN_ID,
};
use test_utils::{test_signer::TestSigner, UniversalAccountController};

static WASM_0_2_0_STATE_PATCH: &[u8] = include_bytes!("./migration/0_2_0_state_patch.borsh");
static WASM_0_4_0_STATE_PATCH: &[u8] = include_bytes!("./migration/0_4_0_state_patch.borsh");

struct PatchKeys {
    passkey: TestSigner,
    ed25519_raw: TestSigner,
    eip191: TestSigner,
    sep53: TestSigner,
}

#[derive(Clone, Copy, Debug)]
enum MigrationSequenceStart {
    Current,
    From0_2_0,
    From0_4_0,
}

#[derive(Clone, Copy, Debug)]
enum MigrationStep {
    V0,
    V1,
    UnbrickV1,
}

fn patch_secret_key() -> [u8; 32] {
    [0x55_u8; 32]
}

fn patch_keys() -> PatchKeys {
    PatchKeys {
        passkey: TestSigner::fixed_passkey(patch_secret_key()),
        ed25519_raw: TestSigner::fixed_ed25519_raw(patch_secret_key()),
        eip191: TestSigner::fixed_eip191(patch_secret_key()),
        sep53: TestSigner::fixed_sep53(patch_secret_key()),
    }
}

/// Deploy a legacy wasm, patch in its borsh state snapshot, then redeploy the
/// current wasm on top — the same staging the `near-workspaces` tests used.
async fn deploy_patched(
    harness: &SandboxHarness,
    wasm: &'static [u8],
    patch: &[u8],
) -> Result<AccountId> {
    deploy_patched_with_version(harness, wasm, patch, None).await
}

async fn deploy_patched_with_version(
    harness: &SandboxHarness,
    wasm: &'static [u8],
    patch: &[u8],
    version: Option<u32>,
) -> Result<AccountId> {
    let ua = ua_id(harness);
    let network = &harness.network;

    deploy_code(network, &ua, test_signer(), wasm.to_vec()).await?;

    let state_patch: HashMap<Vec<u8>, Vec<u8>> = borsh::from_slice(patch)?;
    patch_storage(harness, &ua, state_patch).await?;

    if let Some(version) = version {
        patch_state_version(harness, &ua, &version.to_le_bytes()).await?;
    }

    deploy_code(
        network,
        &ua,
        test_signer(),
        UniversalAccountController::wasm().await.to_vec(),
    )
    .await?;

    Ok(ua)
}

async fn deploy_current(harness: &SandboxHarness, key: KeyId) -> Result<AccountId> {
    let ua = ua_id(harness);
    deploy_with_init(
        &harness.network,
        &ua,
        test_signer(),
        UniversalAccountController::wasm().await.to_vec(),
        "new",
        InitArgs {
            key,
            chain_id: NEAR_TESTNET_CHAIN_ID.into(),
            execute: None,
        },
    )
    .await?;
    Ok(ua)
}

async fn deploy_for_sequence(
    harness: &SandboxHarness,
    start: MigrationSequenceStart,
) -> Result<AccountId> {
    match start {
        MigrationSequenceStart::Current => {
            deploy_current(harness, TestSigner::fixed_passkey([0x44_u8; 32]).id()).await
        }
        MigrationSequenceStart::From0_2_0 => {
            deploy_patched(
                harness,
                UniversalAccountController::wasm_0_2_0(),
                WASM_0_2_0_STATE_PATCH,
            )
            .await
        }
        MigrationSequenceStart::From0_4_0 => {
            deploy_patched(
                harness,
                UniversalAccountController::wasm_0_4_0(),
                WASM_0_4_0_STATE_PATCH,
            )
            .await
        }
    }
}

async fn run_migration_step(
    harness: &SandboxHarness,
    ua: &AccountId,
    step: MigrationStep,
) -> Result<CallOutcome> {
    let args = match step {
        MigrationStep::V0 => state::Migration::from(state::migration::V0 {
            chain_id: U128(NEAR_TESTNET_CHAIN_ID),
        }),
        MigrationStep::V1 => state::Migration::from(state::migration::V1),
        MigrationStep::UnbrickV1 => state::Migration::from(state::migration::UnbrickV1),
    };
    migrate(&harness.network, ua, args).await
}

async fn assert_key_can_increment_counter(
    harness: &SandboxHarness,
    ua: &AccountId,
    ft: &AccountId,
    relayer: &AccountId,
    relayer_signer: Arc<Signer>,
    signer: &TestSigner,
) -> Result<()> {
    let network = &harness.network;
    let key_entry = get_key(network, ua, &signer.id()).await?.unwrap();
    let payload = WithRawString::from_parsed(Payload::new(
        key_entry.next_nonce(),
        vec![Transaction {
            receiver_id: to_sdk(ft),
            actions: vec![increment_action().into()].into(),
        }]
        .into(),
    ));

    execute_as(
        network,
        ua,
        relayer,
        relayer_signer,
        signer.execute_args(payload),
    )
    .await?
    .assert_success();
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn new_account_writes_current_state_version_on_init(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let ua = deploy_current(&harness, TestSigner::fixed_passkey([0x11_u8; 32]).id()).await?;
    let network = &harness.network;

    assert_eq!(target_state_version(network, &ua).await?, 2);
    assert_eq!(stored_state_version(network, &ua).await?, 2);
    assert!(!needs_migration(network, &ua).await?);
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn migration_views_are_exposed(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let ua = deploy_current(&harness, TestSigner::fixed_passkey([0x44_u8; 32]).id()).await?;
    let network = &harness.network;

    assert_eq!(stored_state_version(network, &ua).await?, 2);
    assert_eq!(target_state_version(network, &ua).await?, 2);
    assert!(!needs_migration(network, &ua).await?);
    Ok(())
}

#[rstest]
#[case::current(MigrationSequenceStart::Current)]
#[case::from_0_2_0(MigrationSequenceStart::From0_2_0)]
#[case::from_0_4_0(MigrationSequenceStart::From0_4_0)]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn migrate_can_only_be_called_reflexively(
    #[future(awt)] harness: SandboxHarness,
    #[case] start: MigrationSequenceStart,
) -> Result<()> {
    let ua = deploy_for_sequence(&harness, start).await?;

    let (caller, caller_signer) = create_account(&harness, "caller").await?;

    call(
        &harness.network,
        &ua,
        "migrate",
        state::migration::V0 {
            chain_id: U128(NEAR_TESTNET_CHAIN_ID),
        },
        NearToken::from_near(0),
        Gas::from_tgas(300),
        &caller,
        caller_signer,
    )
    .await?
    .assert_failure_contains("Smart contract panicked: migrate function is private");
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn migrate_accepts_legacy_direct_payload(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let ua = deploy_patched(
        &harness,
        UniversalAccountController::wasm_0_2_0(),
        WASM_0_2_0_STATE_PATCH,
    )
    .await?;
    let network = &harness.network;

    migrate(
        network,
        &ua,
        serde_json::json!({
            "from_version": "v0",
            "chain_id": U128(NEAR_TESTNET_CHAIN_ID),
        }),
    )
    .await?
    .assert_success();

    assert_eq!(stored_state_version(network, &ua).await?, 1);
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn from_0_2_0(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let passkey = patch_keys().passkey;
    let ua = deploy_patched(
        &harness,
        UniversalAccountController::wasm_0_2_0(),
        WASM_0_2_0_STATE_PATCH,
    )
    .await?;
    let network = &harness.network;

    assert_eq!(stored_state_version(network, &ua).await?, 0);
    assert_eq!(target_state_version(network, &ua).await?, 2);
    assert!(needs_migration(network, &ua).await?);

    migrate(
        network,
        &ua,
        state::Migration::from(state::migration::V0 {
            chain_id: U128(NEAR_TESTNET_CHAIN_ID),
        }),
    )
    .await?
    .assert_success();

    assert_eq!(stored_state_version(network, &ua).await?, 1);
    assert_eq!(target_state_version(network, &ua).await?, 2);
    assert!(needs_migration(network, &ua).await?);

    let get_key = get_key(network, &ua, &passkey.id()).await?.unwrap();

    assert_eq!(get_key.chain_id, Some(U128(NEAR_TESTNET_CHAIN_ID)));
    assert_eq!(get_key.index, U64(0));
    assert_eq!(get_key.name, Some("Templar Universal Account".to_string()));
    assert_eq!(get_key.nonce, U64(0));
    assert_eq!(get_key.verifying_contract, to_sdk(&ua));

    migrate(network, &ua, state::Migration::from(state::migration::V1))
        .await?
        .assert_success();

    assert_eq!(stored_state_version(network, &ua).await?, 2);
    assert_eq!(target_state_version(network, &ua).await?, 2);
    assert!(!needs_migration(network, &ua).await?);
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn from_0_2_0_fail_migrate_twice(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let ua = deploy_patched(
        &harness,
        UniversalAccountController::wasm_0_2_0(),
        WASM_0_2_0_STATE_PATCH,
    )
    .await?;

    run_migration_step(&harness, &ua, MigrationStep::V0)
        .await?
        .assert_success();
    run_migration_step(&harness, &ua, MigrationStep::V0)
        .await?
        .assert_failure_contains(
            "Smart contract panicked: Failed to migrate V0: Stored state version 1 != args `from_version` 0",
        );
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn current_state_fail_reinitialize_version(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let ua = deploy_current(&harness, TestSigner::fixed_passkey([0x22_u8; 32]).id()).await?;

    run_migration_step(&harness, &ua, MigrationStep::V1)
        .await?
        .assert_failure_contains(
            "Smart contract panicked: Failed to migrate V1: Stored state version 2 != args `from_version` 1",
        );
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn from_0_4_0_unbrick_v1(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let expected_keys = patch_keys();
    let ua = deploy_patched(
        &harness,
        UniversalAccountController::wasm_0_4_0(),
        WASM_0_4_0_STATE_PATCH,
    )
    .await?;
    let network = &harness.network;
    let ft = common::ft_id(&harness);

    let (relayer, relayer_signer) = create_account(&harness, "relayer").await?;

    assert_eq!(stored_state_version(network, &ua).await?, 0);
    assert_eq!(target_state_version(network, &ua).await?, 2);
    assert!(needs_migration(network, &ua).await?);

    run_migration_step(&harness, &ua, MigrationStep::UnbrickV1)
        .await?
        .assert_success();

    assert_eq!(stored_state_version(network, &ua).await?, 2);
    assert_eq!(target_state_version(network, &ua).await?, 2);
    assert!(!needs_migration(network, &ua).await?);

    let keys = list_keys(network, &ua).await?;
    assert_eq!(keys.len(), 4);
    assert!(keys.contains(&expected_keys.passkey.id()));
    assert!(keys.contains(&expected_keys.ed25519_raw.id()));
    assert!(keys.contains(&expected_keys.eip191.id()));
    assert!(keys.contains(&expected_keys.sep53.id()));

    for key in [
        &expected_keys.passkey,
        &expected_keys.ed25519_raw,
        &expected_keys.eip191,
        &expected_keys.sep53,
    ] {
        let entry = get_key(network, &ua, &key.id()).await?.unwrap();
        assert_eq!(entry.chain_id, Some(U128(NEAR_TESTNET_CHAIN_ID)));
        assert_eq!(entry.name, Some("Templar Universal Account".to_string()));
        assert_eq!(entry.verifying_contract, to_sdk(&ua));
    }

    for signer in [
        &expected_keys.passkey,
        &expected_keys.ed25519_raw,
        &expected_keys.eip191,
        &expected_keys.sep53,
    ] {
        assert_key_can_increment_counter(
            &harness,
            &ua,
            &ft,
            &relayer,
            relayer_signer.clone(),
            signer,
        )
        .await?;
    }

    assert_eq!(get_counter(network, &ft, &ua).await?, 4);
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn from_0_4_0_with_stored_v1_migrates_via_v1(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let ua = deploy_patched_with_version(
        &harness,
        UniversalAccountController::wasm_0_4_0(),
        WASM_0_4_0_STATE_PATCH,
        Some(1),
    )
    .await?;
    let network = &harness.network;

    assert_eq!(stored_state_version(network, &ua).await?, 1);
    assert_eq!(target_state_version(network, &ua).await?, 2);
    assert!(needs_migration(network, &ua).await?);

    run_migration_step(&harness, &ua, MigrationStep::V1)
        .await?
        .assert_success();

    assert_eq!(stored_state_version(network, &ua).await?, 2);
    assert_eq!(target_state_version(network, &ua).await?, 2);
    assert!(!needs_migration(network, &ua).await?);
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn from_0_4_0_fail_unbrick_v1_twice(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let ua = deploy_patched(
        &harness,
        UniversalAccountController::wasm_0_4_0(),
        WASM_0_4_0_STATE_PATCH,
    )
    .await?;

    run_migration_step(&harness, &ua, MigrationStep::UnbrickV1)
        .await?
        .assert_success();
    run_migration_step(&harness, &ua, MigrationStep::UnbrickV1)
        .await?
        .assert_failure_contains(
            "Smart contract panicked: Failed to migrate UnbrickV1: Stored state version 2 != args `from_version` 0",
        );
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn from_0_4_0_fail_v1_migration_without_unbrick(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let ua = deploy_patched(
        &harness,
        UniversalAccountController::wasm_0_4_0(),
        WASM_0_4_0_STATE_PATCH,
    )
    .await?;

    run_migration_step(&harness, &ua, MigrationStep::V1)
        .await?
        .assert_failure_contains(
            "Smart contract panicked: Failed to migrate V1: Stored state version 0 != args `from_version` 1",
        );
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn malformed_stored_version_breaks_public_migration_views(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let ua = deploy_current(&harness, TestSigner::fixed_passkey([0x44_u8; 32]).id()).await?;
    let network = &harness.network;

    patch_state_version(&harness, &ua, &[1, 2, 3]).await?;

    assert!(!view_succeeds(network, &ua, "get_stored_state_version").await);
    assert!(!view_succeeds(network, &ua, "needs_migration").await);
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn future_stored_version_breaks_public_migration_views(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let ua = deploy_current(&harness, TestSigner::fixed_passkey([0x44_u8; 32]).id()).await?;
    let network = &harness.network;

    patch_state_version(&harness, &ua, &9_u32.to_le_bytes()).await?;

    assert!(!view_succeeds(network, &ua, "needs_migration").await);
    Ok(())
}

#[rstest]
#[case::current_then_v0(
    MigrationSequenceStart::Current,
    &[MigrationStep::V0],
    "Failed to migrate V0: Stored state version 2 != args `from_version` 0",
)]
#[case::current_then_unbrick(
    MigrationSequenceStart::Current,
    &[MigrationStep::UnbrickV1],
    "Failed to migrate UnbrickV1: Stored state version 2 != args `from_version` 0",
)]
#[case::from_0_2_0_skip_to_v1(
    MigrationSequenceStart::From0_2_0,
    &[MigrationStep::V1],
    "Failed to migrate V1: Stored state version 0 != args `from_version` 1",
)]
#[case::from_0_2_0_v0_then_unbrick(
    MigrationSequenceStart::From0_2_0,
    &[MigrationStep::V0, MigrationStep::UnbrickV1],
    "Failed to migrate UnbrickV1: Stored state version 1 != args `from_version` 0",
)]
#[case::from_0_2_0_complete_then_v0(
    MigrationSequenceStart::From0_2_0,
    &[MigrationStep::V0, MigrationStep::V1, MigrationStep::V0],
    "Failed to migrate V0: Stored state version 2 != args `from_version` 0",
)]
#[case::from_0_2_0_complete_then_unbrick(
    MigrationSequenceStart::From0_2_0,
    &[MigrationStep::V0, MigrationStep::V1, MigrationStep::UnbrickV1],
    "Failed to migrate UnbrickV1: Stored state version 2 != args `from_version` 0",
)]
#[case::from_0_4_0_unbrick_then_v0(
    MigrationSequenceStart::From0_4_0,
    &[MigrationStep::UnbrickV1, MigrationStep::V0],
    "Failed to migrate V0: Stored state version 2 != args `from_version` 0",
)]
#[case::from_0_4_0_unbrick_then_v1(
    MigrationSequenceStart::From0_4_0,
    &[MigrationStep::UnbrickV1, MigrationStep::V1],
    "Failed to migrate V1: Stored state version 2 != args `from_version` 1",
)]
#[case::from_0_4_0_v0_then_v1(
    MigrationSequenceStart::From0_4_0,
    &[MigrationStep::V0, MigrationStep::V1],
    "Cannot deserialize the contract state.", // Bugged version doesn't have stored state properly set, but still fails correctly.
)]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn invalid_migration_sequences_fail(
    #[future(awt)] harness: SandboxHarness,
    #[case] start: MigrationSequenceStart,
    #[case] steps: &'static [MigrationStep],
    #[case] expected_error: &str,
) -> Result<()> {
    let ua = deploy_for_sequence(&harness, start).await?;

    let mut first_failure = None;
    for &step in steps {
        let outcome = run_migration_step(&harness, &ua, step).await?;
        if !outcome.success {
            first_failure = Some(outcome.failures);
            break;
        }
    }

    let error = first_failure.expect("expected at least one migration failure");
    assert!(error.contains(expected_error), "unexpected error: {error}");
    Ok(())
}
