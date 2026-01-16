use std::collections::HashMap;

use near_sandbox::Sandbox;
use near_sdk::{
    base64::prelude::*,
    json_types::{U128, U64},
};

use templar_universal_account::{
    authentication::passkey::Passkey, contract_state::Migration, NEAR_TESTNET_CHAIN_ID,
};
use test_utils::*;

type StatePatch = HashMap<Vec<u8>, Vec<u8>>;

static WASM_0_2_0_STATE_PATCH: &[u8] = include_bytes!("./migration/0_2_0_state_patch.borsh");

#[rstest::rstest]
#[tokio::test]
pub async fn from_0_2_0(#[future(awt)] worker: Sandbox) {
    let sk = p256::SecretKey::from_bytes(&[0x55u8; 32].into()).unwrap();
    let passkey = Passkey(sk.public_key().into());

    accounts!(worker, universal_account);
    universal_account
        .deploy(UniversalAccountController::wasm_0_2_0().to_vec())
        .await;
    let state_patch: StatePatch = near_sdk::borsh::from_slice(WASM_0_2_0_STATE_PATCH).unwrap();
    worker
        .patch_state(universal_account.id.clone())
        .storage_entries(
            state_patch
                .iter()
                .map(|(k, v)| (BASE64_STANDARD.encode(k), BASE64_STANDARD.encode(v))),
        )
        .send()
        .await
        .unwrap();

    universal_account
        .deploy(UniversalAccountController::wasm().await.to_vec())
        .await;
    let ua = UniversalAccountController {
        account: universal_account,
    };

    assert_eq!(ua.get_stored_state_version().await, 0);
    assert_eq!(ua.get_target_state_version().await, 1);
    assert!(ua.needs_migration().await);

    let r = ua
        .migrate(
            ua.account(),
            Migration::V0 {
                chain_id: U128(NEAR_TESTNET_CHAIN_ID),
            },
        )
        .await;

    for o in r.outcomes() {
        o.clone().into_result().unwrap();
    }

    assert_eq!(ua.get_stored_state_version().await, 1);
    assert_eq!(ua.get_target_state_version().await, 1);
    assert!(!ua.needs_migration().await);

    let get_key = ua.get_key(passkey.clone()).await.unwrap();

    eprintln!("{get_key:?}");

    assert_eq!(get_key.chain_id, Some(U128(NEAR_TESTNET_CHAIN_ID)));
    assert_eq!(get_key.index, U64(0));
    assert_eq!(get_key.name, Some("Templar Universal Account".to_string()));
    assert_eq!(get_key.nonce, U64(0));
    assert_eq!(&get_key.verifying_contract, ua.account().id());
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Stored state version 1 != args `from_version` 0"]
pub async fn from_0_2_0_fail_migrate_twice(#[future(awt)] worker: Sandbox) {
    accounts!(worker, universal_account);
    universal_account
        .deploy(UniversalAccountController::wasm_0_2_0().to_vec())
        .await;
    let state_patch: StatePatch = near_sdk::borsh::from_slice(WASM_0_2_0_STATE_PATCH).unwrap();
    worker
        .patch_state(universal_account.id.clone())
        .storage_entries(
            state_patch
                .iter()
                .map(|(k, v)| (BASE64_STANDARD.encode(k), BASE64_STANDARD.encode(v))),
        )
        .send()
        .await
        .unwrap();

    universal_account
        .deploy(UniversalAccountController::wasm().await.to_vec())
        .await;
    let ua = UniversalAccountController {
        account: universal_account,
    };

    ua.migrate(
        ua.account(),
        Migration::V0 {
            chain_id: U128(NEAR_TESTNET_CHAIN_ID),
        },
    )
    .await;
    ua.migrate(
        ua.account(),
        Migration::V0 {
            chain_id: U128(NEAR_TESTNET_CHAIN_ID),
        },
    )
    .await;
}
