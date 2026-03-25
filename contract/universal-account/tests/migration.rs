#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use near_sdk::json_types::{U128, U64};
use near_workspaces::{network::Sandbox, Worker};
use templar_universal_account::{
    authentication::{
        ed25519::{eip191, raw, sep53},
        passkey,
    },
    state, KeyId, NEAR_TESTNET_CHAIN_ID,
};
use test_utils::{worker, ContractController, UniversalAccountController};

type StatePatch = HashMap<Vec<u8>, Vec<u8>>;

static WASM_0_2_0_STATE_PATCH: &[u8] = include_bytes!("./migration/0_2_0_state_patch.borsh");
static WASM_0_4_0_STATE_PATCH: &[u8] = include_bytes!("./migration/0_4_0_state_patch.borsh");

struct PatchKeys {
    passkey: KeyId,
    ed25519_raw: KeyId,
    eip191: KeyId,
    sep53: KeyId,
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

async fn deploy_patched(
    worker: &Worker<Sandbox>,
    wasm: &'static [u8],
    patch: &[u8],
) -> UniversalAccountController {
    let ua = worker.dev_deploy(wasm).await.unwrap();
    let state_patch: StatePatch = near_sdk::borsh::from_slice(patch).unwrap();

    for (key, value) in state_patch {
        worker.patch_state(ua.id(), &key, &value).await.unwrap();
    }

    let contract = ua
        .as_account()
        .deploy(UniversalAccountController::wasm().await)
        .await
        .unwrap()
        .unwrap();

    UniversalAccountController { contract }
}

async fn deploy_current(worker: &Worker<Sandbox>) -> UniversalAccountController {
    let passkey = passkey::VerifyKey(
        p256::SecretKey::from_bytes(&[0x44_u8; 32].into())
            .unwrap()
            .public_key()
            .into(),
    );

    UniversalAccountController::deploy(
        worker.dev_create_account().await.unwrap(),
        passkey.into(),
        NEAR_TESTNET_CHAIN_ID,
        None,
    )
    .await
}

async fn deploy_for_sequence(
    worker: &Worker<Sandbox>,
    start: MigrationSequenceStart,
) -> UniversalAccountController {
    match start {
        MigrationSequenceStart::Current => deploy_current(worker).await,
        MigrationSequenceStart::From0_2_0 => {
            deploy_patched(
                worker,
                UniversalAccountController::wasm_0_2_0(),
                WASM_0_2_0_STATE_PATCH,
            )
            .await
        }
        MigrationSequenceStart::From0_4_0 => {
            deploy_patched(
                worker,
                UniversalAccountController::wasm_0_4_0(),
                WASM_0_4_0_STATE_PATCH,
            )
            .await
        }
    }
}

async fn run_migration_step(
    ua: &UniversalAccountController,
    step: MigrationStep,
) -> (bool, String) {
    let result = match step {
        MigrationStep::V0 => ua
            .contract()
            .as_account()
            .call(ua.contract().id(), "migrate")
            .args_json(near_sdk::serde_json::json!({
                "args": state::Migration::from(state::migration::V0 {
                    chain_id: U128(NEAR_TESTNET_CHAIN_ID),
                }),
            }))
            .max_gas()
            .transact()
            .await
            .unwrap(),
        MigrationStep::V1 => ua
            .contract()
            .as_account()
            .call(ua.contract().id(), "migrate")
            .args_json(near_sdk::serde_json::json!({
                "args": state::Migration::from(state::migration::V1),
            }))
            .max_gas()
            .transact()
            .await
            .unwrap(),
        MigrationStep::UnbrickV1 => ua
            .contract()
            .as_account()
            .call(ua.contract().id(), "migrate")
            .args_json(near_sdk::serde_json::json!({
                "args": state::Migration::from(state::migration::UnbrickV1),
            }))
            .max_gas()
            .transact()
            .await
            .unwrap(),
    };

    (
        result.failures().is_empty(),
        format!("{:#?}", result.failures()),
    )
}

fn patch_secret_key() -> [u8; 32] {
    [0x55_u8; 32]
}

fn patch_keys() -> PatchKeys {
    let passkey_secret = p256::SecretKey::from_bytes(&patch_secret_key().into()).unwrap();
    let ed25519_secret = ed25519_dalek::SigningKey::from_bytes(&patch_secret_key());
    let eip191_secret =
        alloy::signers::local::PrivateKeySigner::from_bytes(&patch_secret_key().into()).unwrap();

    PatchKeys {
        passkey: passkey::VerifyKey(passkey_secret.public_key().into()).into(),
        ed25519_raw: raw::VerifyKey(ed25519_secret.verifying_key().to_bytes().into()).into(),
        eip191: eip191::VerifyKey(eip191_secret.address().into()).into(),
        sep53: sep53::VerifyKey(ed25519_secret.verifying_key().to_bytes().into()).into(),
    }
}

#[rstest::rstest]
#[tokio::test]
pub async fn new_account_writes_current_state_version_on_init(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    let sk = p256::SecretKey::from_bytes(&[0x11u8; 32].into()).unwrap();
    let passkey = passkey::VerifyKey(sk.public_key().into());

    let ua = UniversalAccountController::deploy(
        worker.dev_create_account().await.unwrap(),
        passkey.into(),
        NEAR_TESTNET_CHAIN_ID,
        None,
    )
    .await;

    assert_eq!(ua.migrate_target_state_version().await, 2);
    assert_eq!(ua.migrate_stored_state_version().await, 2);
    assert!(!ua.migrate_needs_migration().await);
}

#[rstest::rstest]
#[tokio::test]
pub async fn from_0_2_0(#[future(awt)] worker: Worker<Sandbox>) {
    let passkey = passkey::VerifyKey(
        p256::SecretKey::from_bytes(&patch_secret_key().into())
            .unwrap()
            .public_key()
            .into(),
    );
    let ua = deploy_patched(
        &worker,
        UniversalAccountController::wasm_0_2_0(),
        WASM_0_2_0_STATE_PATCH,
    )
    .await;

    assert_eq!(ua.migrate_stored_state_version().await, 0);
    assert_eq!(ua.migrate_target_state_version().await, 2);
    assert!(ua.migrate_needs_migration().await);

    let r = ua
        .migrate(
            ua.contract().as_account(),
            state::migration::V0 {
                chain_id: U128(NEAR_TESTNET_CHAIN_ID),
            },
        )
        .await;

    for o in r.outcomes() {
        o.clone().into_result().unwrap();
    }

    assert_eq!(ua.migrate_stored_state_version().await, 1);
    assert_eq!(ua.migrate_target_state_version().await, 2);
    assert!(ua.migrate_needs_migration().await);

    let get_key = ua.get_key(passkey.clone()).await.unwrap();

    eprintln!("{get_key:?}");

    assert_eq!(get_key.chain_id, Some(U128(NEAR_TESTNET_CHAIN_ID)));
    assert_eq!(get_key.index, U64(0));
    assert_eq!(get_key.name, Some("Templar Universal Account".to_string()));
    assert_eq!(get_key.nonce, U64(0));
    assert_eq!(&get_key.verifying_contract, ua.contract().as_account().id());

    let r = ua
        .migrate(ua.contract().as_account(), state::migration::V1)
        .await;

    for o in r.outcomes() {
        o.clone().into_result().unwrap();
    }

    assert_eq!(ua.migrate_stored_state_version().await, 2);
    assert_eq!(ua.migrate_target_state_version().await, 2);
    assert!(!ua.migrate_needs_migration().await);
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Failed to migrate V0: Stored state version 1 != args `from_version` 0"]
pub async fn from_0_2_0_fail_migrate_twice(#[future(awt)] worker: Worker<Sandbox>) {
    let ua = deploy_patched(
        &worker,
        UniversalAccountController::wasm_0_2_0(),
        WASM_0_2_0_STATE_PATCH,
    )
    .await;

    ua.migrate(
        ua.contract().as_account(),
        state::migration::V0 {
            chain_id: U128(NEAR_TESTNET_CHAIN_ID),
        },
    )
    .await;
    ua.migrate(
        ua.contract().as_account(),
        state::migration::V0 {
            chain_id: U128(NEAR_TESTNET_CHAIN_ID),
        },
    )
    .await;
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Failed to migrate V1: Stored state version 2 != args `from_version` 1"]
pub async fn current_state_fail_reinitialize_version(#[future(awt)] worker: Worker<Sandbox>) {
    let sk = p256::SecretKey::from_bytes(&[0x22u8; 32].into()).unwrap();
    let passkey = passkey::VerifyKey(sk.public_key().into());

    let ua = UniversalAccountController::deploy(
        worker.dev_create_account().await.unwrap(),
        passkey.into(),
        NEAR_TESTNET_CHAIN_ID,
        None,
    )
    .await;

    ua.migrate(ua.contract().as_account(), state::migration::V1)
        .await;
}

#[rstest::rstest]
#[tokio::test]
pub async fn from_0_4_0_unbrick_v1(#[future(awt)] worker: Worker<Sandbox>) {
    let expected_keys = patch_keys();
    let ua = deploy_patched(
        &worker,
        UniversalAccountController::wasm_0_4_0(),
        WASM_0_4_0_STATE_PATCH,
    )
    .await;

    assert_eq!(ua.migrate_stored_state_version().await, 0);
    assert_eq!(ua.migrate_target_state_version().await, 2);
    assert!(ua.migrate_needs_migration().await);

    let result = ua
        .migrate(ua.contract().as_account(), state::migration::UnbrickV1)
        .await;

    for outcome in result.outcomes() {
        outcome.clone().into_result().unwrap();
    }

    assert_eq!(ua.migrate_stored_state_version().await, 2);
    assert_eq!(ua.migrate_target_state_version().await, 2);
    assert!(!ua.migrate_needs_migration().await);

    let keys = ua.list_keys(None, None).await;
    assert_eq!(keys.len(), 4);

    assert!(keys.contains(&expected_keys.passkey));
    assert!(keys.contains(&expected_keys.ed25519_raw));
    assert!(keys.contains(&expected_keys.eip191));
    assert!(keys.contains(&expected_keys.sep53));
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Failed to migrate UnbrickV1: Stored state version 2 != args `from_version` 0"]
pub async fn from_0_4_0_fail_unbrick_v1_twice(#[future(awt)] worker: Worker<Sandbox>) {
    let ua = deploy_patched(
        &worker,
        UniversalAccountController::wasm_0_4_0(),
        WASM_0_4_0_STATE_PATCH,
    )
    .await;

    ua.migrate(ua.contract().as_account(), state::migration::UnbrickV1)
        .await;
    ua.migrate(ua.contract().as_account(), state::migration::UnbrickV1)
        .await;
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Failed to migrate V1: Stored state version 0 != args `from_version` 1"]
pub async fn from_0_4_0_fail_v1_migration_without_unbrick(#[future(awt)] worker: Worker<Sandbox>) {
    let ua = deploy_patched(
        &worker,
        UniversalAccountController::wasm_0_4_0(),
        WASM_0_4_0_STATE_PATCH,
    )
    .await;

    ua.migrate(ua.contract().as_account(), state::migration::V1)
        .await;
}

#[rstest::rstest]
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
    "Cannot deserialize the contract state.",
)]
#[tokio::test]
pub async fn invalid_migration_sequences_fail(
    #[future(awt)] worker: Worker<Sandbox>,
    #[case] start: MigrationSequenceStart,
    #[case] steps: &'static [MigrationStep],
    #[case] expected_error: &str,
) {
    let ua = deploy_for_sequence(&worker, start).await;

    let mut results = Vec::new();
    for &step in steps {
        results.push(run_migration_step(&ua, step).await);
    }

    let failing = results
        .into_iter()
        .find(|(ok, _)| !ok)
        .expect("expected at least one migration failure");

    let error = failing.1;
    assert!(error.contains(expected_error), "unexpected error: {error}");
}
