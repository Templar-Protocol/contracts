#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use near_sdk::{
    borsh,
    json_types::{U128, U64},
    serde_json::json,
    NearToken,
};
use near_workspaces::{network::Sandbox, Account, Worker};
use templar_universal_account::{
    authentication::{with_raw_string::WithRawString, Payload},
    state,
    transaction::{FunctionCallAction, Transaction},
    NEAR_TESTNET_CHAIN_ID,
};
use test_utils::{
    assert_all_outcomes_success, controller::migration::MigrationController,
    test_signer::TestSigner, worker, ContractController, FtController, UniversalAccountController,
};

type StatePatch = HashMap<Vec<u8>, Vec<u8>>;

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

async fn deploy_patched(
    worker: &Worker<Sandbox>,
    wasm: &'static [u8],
    patch: &[u8],
) -> UniversalAccountController {
    deploy_patched_with_version(worker, wasm, patch, None).await
}

async fn deploy_patched_with_version(
    worker: &Worker<Sandbox>,
    wasm: &'static [u8],
    patch: &[u8],
    version: Option<u32>,
) -> UniversalAccountController {
    let ua = worker.dev_deploy(wasm).await.unwrap();
    let state_patch: StatePatch = borsh::from_slice(patch).unwrap();

    for (key, value) in state_patch {
        worker.patch_state(ua.id(), &key, &value).await.unwrap();
    }

    if let Some(version) = version {
        worker
            .patch_state(ua.id(), b"__v", &version.to_le_bytes())
            .await
            .unwrap();
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
    let passkey = TestSigner::fixed_passkey([0x44_u8; 32]).id();

    UniversalAccountController::deploy(
        worker.dev_create_account().await.unwrap(),
        passkey,
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
) -> Result<(), String> {
    let args = match step {
        MigrationStep::V0 => state::Migration::from(state::migration::V0 {
            chain_id: U128(NEAR_TESTNET_CHAIN_ID),
        }),
        MigrationStep::V1 => state::Migration::from(state::migration::V1),
        MigrationStep::UnbrickV1 => state::Migration::from(state::migration::UnbrickV1),
    };

    let result = ua
        .contract()
        .as_account()
        .call(ua.contract().id(), "migrate")
        .args_json(args)
        .max_gas()
        .transact()
        .await
        .unwrap();

    let errs = format!("{:#?}", result.failures());

    if result.failures().is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

fn increment() -> FunctionCallAction {
    FunctionCallAction {
        function_name: "increment".to_string(),
        arguments: json!({}).to_string().into_bytes().into(),
        amount: NearToken::from_near(0),
        gas: near_sdk::Gas::from_tgas(30),
    }
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

async fn assert_key_can_increment_counter(
    ua: &UniversalAccountController,
    ft: &FtController,
    third_party: &Account,
    signer: &TestSigner,
) {
    let key = signer.id();
    let key_entry = ua.get_key(key).await.unwrap();
    let payload = WithRawString::from_parsed(Payload::new(
        key_entry.next_nonce(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![increment().into()].into(),
        }]
        .into(),
    ));

    let result = ua.execute(third_party, signer.execute_args(payload)).await;

    assert_all_outcomes_success(&result);
}

#[rstest::rstest]
#[tokio::test]
pub async fn new_account_writes_current_state_version_on_init(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    let passkey = TestSigner::fixed_passkey([0x11_u8; 32]).id();

    let ua = UniversalAccountController::deploy(
        worker.dev_create_account().await.unwrap(),
        passkey,
        NEAR_TESTNET_CHAIN_ID,
        None,
    )
    .await;

    assert_eq!(ua.get_target_state_version().await, 2);
    assert_eq!(ua.get_stored_state_version().await, 2);
    assert!(!ua.needs_migration().await);
}

#[rstest::rstest]
#[tokio::test]
pub async fn migration_views_are_exposed(#[future(awt)] worker: Worker<Sandbox>) {
    let ua = deploy_current(&worker).await;

    assert_eq!(ua.get_stored_state_version().await, 2);
    assert_eq!(ua.get_target_state_version().await, 2);
    assert!(!ua.needs_migration().await);
}

#[rstest::rstest]
#[tokio::test]
pub async fn migrate_accepts_legacy_direct_payload(#[future(awt)] worker: Worker<Sandbox>) {
    let ua = deploy_patched(
        &worker,
        UniversalAccountController::wasm_0_2_0(),
        WASM_0_2_0_STATE_PATCH,
    )
    .await;

    let result = ua
        .contract()
        .as_account()
        .call(ua.contract().id(), "migrate")
        .args_json(json!({
            "from_version": "v0",
            "chain_id": U128(NEAR_TESTNET_CHAIN_ID),
        }))
        .max_gas()
        .transact()
        .await
        .unwrap()
        .unwrap();

    assert_all_outcomes_success(&result);
    assert_eq!(ua.get_stored_state_version().await, 1);
}

#[rstest::rstest]
#[tokio::test]
pub async fn from_0_2_0(#[future(awt)] worker: Worker<Sandbox>) {
    let passkey = patch_keys().passkey;
    let ua = deploy_patched(
        &worker,
        UniversalAccountController::wasm_0_2_0(),
        WASM_0_2_0_STATE_PATCH,
    )
    .await;

    assert_eq!(ua.get_stored_state_version().await, 0);
    assert_eq!(ua.get_target_state_version().await, 2);
    assert!(ua.needs_migration().await);

    let r = ua
        .migrate(
            ua.contract().as_account(),
            state::migration::V0 {
                chain_id: U128(NEAR_TESTNET_CHAIN_ID),
            },
        )
        .await;

    assert_all_outcomes_success(&r);

    assert_eq!(ua.get_stored_state_version().await, 1);
    assert_eq!(ua.get_target_state_version().await, 2);
    assert!(ua.needs_migration().await);

    let get_key = ua.get_key(passkey.id()).await.unwrap();

    eprintln!("{get_key:?}");

    assert_eq!(get_key.chain_id, Some(U128(NEAR_TESTNET_CHAIN_ID)));
    assert_eq!(get_key.index, U64(0));
    assert_eq!(get_key.name, Some("Templar Universal Account".to_string()));
    assert_eq!(get_key.nonce, U64(0));
    assert_eq!(&get_key.verifying_contract, ua.contract().as_account().id());

    let r = ua
        .migrate(ua.contract().as_account(), state::migration::V1)
        .await;

    assert_all_outcomes_success(&r);

    assert_eq!(ua.get_stored_state_version().await, 2);
    assert_eq!(ua.get_target_state_version().await, 2);
    assert!(!ua.needs_migration().await);
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
    let passkey = TestSigner::fixed_passkey([0x22_u8; 32]).id();

    let ua = UniversalAccountController::deploy(
        worker.dev_create_account().await.unwrap(),
        passkey,
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
    test_utils::accounts!(worker, ft_account, third_party);
    let ua = deploy_patched(
        &worker,
        UniversalAccountController::wasm_0_4_0(),
        WASM_0_4_0_STATE_PATCH,
    )
    .await;

    assert_eq!(ua.get_stored_state_version().await, 0);
    assert_eq!(ua.get_target_state_version().await, 2);
    assert!(ua.needs_migration().await);

    let result = ua
        .migrate(ua.contract().as_account(), state::migration::UnbrickV1)
        .await;

    assert_all_outcomes_success(&result);

    assert_eq!(ua.get_stored_state_version().await, 2);
    assert_eq!(ua.get_target_state_version().await, 2);
    assert!(!ua.needs_migration().await);

    let keys = ua.list_keys(None, None).await;
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
        let entry = ua.get_key(key.id()).await.unwrap();
        assert_eq!(entry.chain_id, Some(U128(NEAR_TESTNET_CHAIN_ID)));
        assert_eq!(entry.name, Some("Templar Universal Account".to_string()));
        assert_eq!(&entry.verifying_contract, ua.contract().as_account().id());
    }

    let ft = FtController::deploy(ft_account, "Fungible Token", "FT").await;
    for signer in [
        &expected_keys.passkey,
        &expected_keys.ed25519_raw,
        &expected_keys.eip191,
        &expected_keys.sep53,
    ] {
        assert_key_can_increment_counter(&ua, &ft, &third_party, signer).await;
    }

    assert_eq!(ft.get_counter(ua.contract().id()).await, 4);
}

#[rstest::rstest]
#[tokio::test]
pub async fn from_0_4_0_with_stored_v1_migrates_via_v1(#[future(awt)] worker: Worker<Sandbox>) {
    let ua = deploy_patched_with_version(
        &worker,
        UniversalAccountController::wasm_0_4_0(),
        WASM_0_4_0_STATE_PATCH,
        Some(1),
    )
    .await;

    assert_eq!(ua.get_stored_state_version().await, 1);
    assert_eq!(ua.get_target_state_version().await, 2);
    assert!(ua.needs_migration().await);

    let result = ua
        .migrate(ua.contract().as_account(), state::migration::V1)
        .await;

    assert_all_outcomes_success(&result);

    assert_eq!(ua.get_stored_state_version().await, 2);
    assert_eq!(ua.get_target_state_version().await, 2);
    assert!(!ua.needs_migration().await);
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
#[tokio::test]
pub async fn malformed_stored_version_breaks_public_migration_views(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    let ua = deploy_current(&worker).await;

    worker
        .patch_state(ua.contract().id(), b"__v", &[1, 2, 3])
        .await
        .unwrap();

    assert!(ua
        .contract()
        .view("get_stored_state_version")
        .args_json(json!({}))
        .await
        .is_err());
    assert!(ua
        .contract()
        .view("needs_migration")
        .args_json(json!({}))
        .await
        .is_err());
}

#[rstest::rstest]
#[tokio::test]
pub async fn future_stored_version_breaks_public_migration_views(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    let ua = deploy_current(&worker).await;

    worker
        .patch_state(ua.contract().id(), b"__v", &9_u32.to_le_bytes())
        .await
        .unwrap();

    assert!(ua
        .contract()
        .view("needs_migration")
        .args_json(json!({}))
        .await
        .is_err());
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
        .find_map(|r| r.err())
        .expect("expected at least one migration failure");

    let error = failing;
    assert!(error.contains(expected_error), "unexpected error: {error}");
}
