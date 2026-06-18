#![allow(clippy::unwrap_used)]

use near_sdk::{
    borsh, env,
    json_types::{U128, U64},
    serde_json::{self, json},
    NearToken,
};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_universal_account::{
    authentication::{with_raw_string::WithRawString, Payload},
    state,
    transaction::{FunctionCallAction, Transaction},
    ExecuteArgs, KeyParameters, PayloadExecutionParameters, NEAR_TESTNET_CHAIN_ID,
};
use test_utils::{
    assert_all_outcomes_success,
    controller::{migration::MigrationController, universal_account::UniversalAccountController},
    test_signer::TestSigner,
    worker, ContractController, FtController, StorageManagementController,
};

fn mint(amount: u128) -> FunctionCallAction {
    FunctionCallAction {
        function_name: "mint".to_string(),
        arguments: serde_json::to_vec(&json!({
            "amount": U128(amount),
        }))
        .unwrap()
        .into(),
        amount: NearToken::from_near(0),
        gas: near_sdk::Gas::from_tgas(30),
    }
}

struct Setup {
    uac: UniversalAccountController,
    ft: FtController,
    third_party: near_workspaces::Account,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ExecuteOnCreate {
    None,
    Empty,
    Counter,
}

async fn setup(
    worker: &Worker<Sandbox>,
    sk: &TestSigner,
    migrated: bool,
    execute_on_create: ExecuteOnCreate,
) -> Setup {
    test_utils::accounts!(worker, uni_account, ft_account, third_party);

    let ft_account_id = ft_account.id().to_owned();

    let make_uac = || async move {
        if migrated {
            let c = uni_account
                .deploy(UniversalAccountController::wasm_0_2_0())
                .await
                .unwrap()
                .unwrap();
            c.call("new")
                .args_json(json!({
                    "key": sk.id(),
                }))
                .transact()
                .await
                .unwrap()
                .unwrap();

            let ua = uni_account
                .deploy(UniversalAccountController::wasm().await)
                .await
                .unwrap()
                .unwrap();

            let ua = UniversalAccountController { contract: ua };

            let r = ua
                .migrate(
                    ua.contract().as_account(),
                    state::migration::V0 {
                        chain_id: U128(NEAR_TESTNET_CHAIN_ID),
                    },
                )
                .await;

            assert_all_outcomes_success(&r);

            let r = ua
                .migrate(ua.contract().as_account(), state::migration::V1)
                .await;

            assert_all_outcomes_success(&r);

            ua
        } else {
            let execute = match execute_on_create {
                ExecuteOnCreate::None => None,
                ExecuteOnCreate::Empty => Some(vec![]),
                ExecuteOnCreate::Counter => Some(vec![Transaction {
                    receiver_id: ft_account_id,
                    actions: vec![FunctionCallAction::new(
                        "increment",
                        b"{}",
                        NearToken::from_near(0),
                        near_sdk::Gas::from_tgas(3),
                    )
                    .into()]
                    .into(),
                }]),
            };
            UniversalAccountController::deploy(uni_account, sk.id(), NEAR_TESTNET_CHAIN_ID, execute)
                .await
        }
    };

    let ft = FtController::deploy(ft_account, "Fungible Token", "FT").await;
    let uac = make_uac().await;

    let counter = ft.get_counter(uac.contract.id()).await;
    if execute_on_create == ExecuteOnCreate::Counter && !migrated {
        assert_eq!(counter, 1);
    } else {
        assert_eq!(counter, 0);
    }

    ft.storage_deposit_for(
        &third_party,
        uac.contract().id(),
        NearToken::from_near(1).saturating_div(4),
    )
    .await;

    Setup {
        uac,
        ft,
        third_party,
    }
}

fn signed_mint_execute_args(
    sk: &TestSigner,
    ft: &FtController,
    parameters: PayloadExecutionParameters,
    amount: u128,
) -> ExecuteArgs<Box<[Transaction]>> {
    let payload = WithRawString::from_parsed(Payload::new(
        parameters,
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(amount).into()].into(),
        }]
        .into(),
    ));

    sk.execute_args(payload)
}

#[rstest]
#[tokio::test]
pub async fn universal_account(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(
        (TestSigner::random_passkey(), false),
        (TestSigner::random_passkey(), true),
        (TestSigner::random_ed25519_raw(), false),
        (TestSigner::random_ed25519_raw(), true),
        (TestSigner::random_eip712(), false),
        (TestSigner::random_sep53(), false),
        (TestSigner::random_eip191(), false),
    )]
    (sk, migrated): (TestSigner, bool),
    #[values(
        ExecuteOnCreate::None,
        ExecuteOnCreate::Empty,
        ExecuteOnCreate::Counter
    )]
    execute_on_create: ExecuteOnCreate,
) {
    let Setup {
        uac,
        ft,
        third_party,
    } = setup(&worker, &sk, migrated, execute_on_create).await;

    let key_list = uac.list_keys(None, None).await;
    assert_eq!(
        key_list,
        vec![sk.id()],
        "Key should be the only one in control of the account immediately after deployment"
    );

    let key_entry = uac.get_key(sk.id()).await.unwrap();
    let block_height = key_entry.block_height;

    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 0);

    let payload = WithRawString::from_parsed(Payload::new(
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .with_key_parameters(KeyParameters {
                block_height,
                index: U64(0),
                nonce: U64(1),
            })
            .verifying_contract(uac.contract().id().clone())
            .build_salt(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    ));

    let execute_args = sk.execute_args(payload);

    let e = uac.execute(&third_party, execute_args).await;

    assert_all_outcomes_success(&e);

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should succeed");

    let key_entry = uac.get_key(sk.id()).await.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 1);
    assert_eq!(key_entry.chain_id, Some(NEAR_TESTNET_CHAIN_ID.into()));
    assert_eq!(key_entry.name, Some("Templar Universal Account".into()));
    assert_eq!(&key_entry.verifying_contract, uac.contract().id());
    assert_eq!(key_entry.version, Some("1.2.1".into()));
    assert_eq!(
        key_entry.salt,
        Some(
            env::keccak256_array(
                borsh::to_vec(&(key_entry.block_height, key_entry.index)).unwrap()
            )
            .into()
        )
    );

    // Second execution, check nonce advancement

    let payload = WithRawString::from_parsed(Payload::new(
        key_entry.next_nonce(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    ));

    let execute_args = sk.execute_args(payload);

    let e = uac.execute(&third_party, execute_args).await;

    assert_all_outcomes_success(&e);

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 200, "Function call should succeed");

    let key_entry = uac.get_key(sk.id()).await.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 2);
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Execution parameter `nonce` mismatch: expected `2`, got `3`"]
async fn skip_nonce(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(
        (TestSigner::random_passkey(), false),
        (TestSigner::random_passkey(), true),
        (TestSigner::random_ed25519_raw(), false),
        (TestSigner::random_ed25519_raw(), true),
        (TestSigner::random_eip712(), false),
        (TestSigner::random_sep53(), false),
        (TestSigner::random_eip191(), false),
    )]
    (sk, migrated): (TestSigner, bool),
    #[values(
        ExecuteOnCreate::None,
        ExecuteOnCreate::Empty,
        ExecuteOnCreate::Counter
    )]
    execute_on_create: ExecuteOnCreate,
) {
    let Setup {
        uac,
        ft,
        third_party,
    } = setup(&worker, &sk, migrated, execute_on_create).await;

    let key_entry = uac.get_key(sk.id()).await.unwrap();
    let block_height = key_entry.block_height;

    let payload = WithRawString::from_parsed(Payload::new(
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .with_key_parameters(KeyParameters {
                block_height,
                index: U64(0),
                nonce: U64(1),
            })
            .verifying_contract(uac.contract().id().clone())
            .build_salt(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    ));

    let execute_args = sk.execute_args(payload);

    uac.execute(&third_party, execute_args).await;

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should not succeed");

    let key_entry = uac.get_key(sk.id()).await.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 1);

    // Try to skip a nonce

    let payload = WithRawString::from_parsed(Payload::new(
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .with_key_parameters(KeyParameters {
                block_height,
                index: U64(0),
                nonce: U64(3),
            })
            .verifying_contract(uac.contract().id().clone())
            .build_salt(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    ));

    let execute_args = sk.execute_args(payload);

    uac.execute(&third_party, execute_args).await;
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Execution parameter `nonce` mismatch: expected `2`, got `1`"]
async fn reuse_nonce(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(
        (TestSigner::random_passkey(), false),
        (TestSigner::random_passkey(), true),
        (TestSigner::random_ed25519_raw(), false),
        (TestSigner::random_ed25519_raw(), true),
        (TestSigner::random_eip712(), false),
        (TestSigner::random_sep53(), false),
        (TestSigner::random_eip191(), false),
    )]
    (sk, migrated): (TestSigner, bool),
    #[values(
        ExecuteOnCreate::None,
        ExecuteOnCreate::Empty,
        ExecuteOnCreate::Counter
    )]
    execute_on_create: ExecuteOnCreate,
) {
    let Setup {
        uac,
        ft,
        third_party,
    } = setup(&worker, &sk, migrated, execute_on_create).await;

    let key_entry = uac.get_key(sk.id()).await.unwrap();
    let block_height = key_entry.block_height;

    let payload = WithRawString::from_parsed(Payload::new(
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .with_key_parameters(KeyParameters {
                block_height,
                index: U64(0),
                nonce: U64(1),
            })
            .verifying_contract(uac.contract().id().clone())
            .build_salt(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    ));

    let execute_args = sk.execute_args(payload);

    uac.execute(&third_party, execute_args).await;

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should succeed");

    let key_entry = uac.get_key(sk.id()).await.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 1);

    // Try to reuse a nonce

    let payload = WithRawString::from_parsed(Payload::new(
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .with_key_parameters(KeyParameters {
                block_height,
                index: U64(0),
                nonce: U64(1),
            })
            .verifying_contract(uac.contract().id().clone())
            .build_salt(),
        vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![mint(100).into()].into(),
        }]
        .into(),
    ));

    let execute_args = sk.execute_args(payload);

    uac.execute(&third_party, execute_args).await;
}

#[tokio::test]
async fn failed_execute_does_not_consume_nonce_and_success_consumes_once() {
    let worker = worker().await;
    let sk = TestSigner::random_passkey();
    let Setup {
        uac,
        ft,
        third_party,
    } = setup(&worker, &sk, false, ExecuteOnCreate::None).await;

    let key_entry = uac.get_key(sk.id()).await.unwrap();
    assert_eq!(key_entry.nonce.0, 0);

    let mut skipped_nonce = key_entry.clone();
    skipped_nonce.nonce = U64(2);
    let execute_args = signed_mint_execute_args(&sk, &ft, skipped_nonce, 100);

    let result = third_party
        .call(uac.contract().id(), "execute")
        .args_json(json!({ "args": execute_args }))
        .gas(near_sdk::Gas::from_tgas(300))
        .transact()
        .await
        .unwrap();
    assert!(
        result.is_failure(),
        "skipped nonce execution should fail: {:?}",
        result.failures(),
    );

    let key_entry_after_failure = uac.get_key(sk.id()).await.unwrap();
    assert_eq!(
        key_entry_after_failure.nonce.0, 0,
        "failed verification must roll back the pre-verification nonce increment",
    );
    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 0, "failed execution should not mint");

    let execute_args =
        signed_mint_execute_args(&sk, &ft, key_entry_after_failure.next_nonce(), 100);
    let result = uac.execute(&third_party, execute_args).await;
    assert_all_outcomes_success(&result);

    let key_entry_after_success = uac.get_key(sk.id()).await.unwrap();
    assert_eq!(
        key_entry_after_success.nonce.0, 1,
        "successful execution must consume exactly one nonce",
    );
    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "successful execution should mint once");
}

#[tokio::test]
async fn replayed_nonce_fails_without_reexecuting_payload() {
    let worker = worker().await;
    let sk = TestSigner::random_passkey();
    let Setup {
        uac,
        ft,
        third_party,
    } = setup(&worker, &sk, false, ExecuteOnCreate::None).await;

    let key_entry = uac.get_key(sk.id()).await.unwrap();
    let execute_args = signed_mint_execute_args(&sk, &ft, key_entry.next_nonce(), 100);

    let result = uac.execute(&third_party, execute_args.clone()).await;
    assert_all_outcomes_success(&result);

    let key_entry_after_success = uac.get_key(sk.id()).await.unwrap();
    assert_eq!(
        key_entry_after_success.nonce.0, 1,
        "successful execution must consume the signed nonce",
    );
    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "payload should execute once");

    let result = third_party
        .call(uac.contract().id(), "execute")
        .args_json(json!({ "args": execute_args }))
        .gas(near_sdk::Gas::from_tgas(300))
        .transact()
        .await
        .unwrap();
    assert!(
        result.is_failure(),
        "replayed nonce execution should fail: {:?}",
        result.failures(),
    );

    let key_entry_after_replay = uac.get_key(sk.id()).await.unwrap();
    assert_eq!(
        key_entry_after_replay.nonce.0, 1,
        "failed replay must not advance the nonce",
    );
    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "replayed payload must not execute again");
}

#[tokio::test]
async fn key_indexes_are_unique_across_remove_and_readd() {
    let worker = worker().await;
    let sk1 = TestSigner::random_passkey();
    let sk2 = TestSigner::random_ed25519_raw();
    let Setup { uac, .. } = setup(&worker, &sk1, false, ExecuteOnCreate::None).await;

    let key1 = sk1.id();
    let key2 = sk2.id();

    let initial_entry = uac.get_key(key1.clone()).await.unwrap();
    assert_eq!(initial_entry.index.0, 0);

    let result = uac.add_key(uac.contract().as_account(), key2.clone()).await;
    assert_all_outcomes_success(&result);
    let second_entry = uac.get_key(key2.clone()).await.unwrap();
    assert_eq!(second_entry.index.0, 1);

    let result = uac
        .remove_key(uac.contract().as_account(), key1.clone())
        .await;
    assert_all_outcomes_success(&result);
    assert!(uac.get_key(key1.clone()).await.is_none());

    let result = uac.add_key(uac.contract().as_account(), key1.clone()).await;
    assert_all_outcomes_success(&result);
    let readded_entry = uac.get_key(key1.clone()).await.unwrap();
    assert_eq!(
        readded_entry.index.0, 2,
        "re-added keys must receive a fresh monotonic index",
    );
    assert_eq!(readded_entry.nonce.0, 0);

    let keys = uac.list_keys(None, None).await;
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&key1));
    assert!(keys.contains(&key2));
}

#[tokio::test]
async fn cannot_remove_last_key() {
    let worker = worker().await;
    let sk = TestSigner::random_passkey();
    let Setup { uac, .. } = setup(&worker, &sk, false, ExecuteOnCreate::None).await;

    let result = uac
        .contract()
        .as_account()
        .call(uac.contract().id(), "remove_key")
        .args_json(json!({ "key": sk.id() }))
        .gas(near_sdk::Gas::from_tgas(30))
        .transact()
        .await
        .unwrap();
    assert!(
        result.is_failure(),
        "last-key removal should fail: {:?}",
        result.failures(),
    );

    let keys = uac.list_keys(None, None).await;
    assert_eq!(keys, vec![sk.id()]);
    assert!(uac.get_key(sk.id()).await.is_some());
}

#[tokio::test]
async fn removed_key_cannot_execute_transaction() {
    let worker = worker().await;
    let removed_sk = TestSigner::random_passkey();
    let retained_sk = TestSigner::random_ed25519_raw();
    let Setup {
        uac,
        ft,
        third_party,
    } = setup(&worker, &removed_sk, false, ExecuteOnCreate::None).await;

    let removed_key = removed_sk.id();
    let retained_key = retained_sk.id();

    let result = uac
        .add_key(uac.contract().as_account(), retained_key.clone())
        .await;
    assert_all_outcomes_success(&result);

    let removed_entry_before = uac.get_key(removed_key.clone()).await.unwrap();
    let result = uac
        .remove_key(uac.contract().as_account(), removed_key.clone())
        .await;
    assert_all_outcomes_success(&result);
    assert!(uac.get_key(removed_key.clone()).await.is_none());

    let execute_args =
        signed_mint_execute_args(&removed_sk, &ft, removed_entry_before.next_nonce(), 100);
    let result = third_party
        .call(uac.contract().id(), "execute")
        .args_json(json!({ "args": execute_args }))
        .gas(near_sdk::Gas::from_tgas(300))
        .transact()
        .await
        .unwrap();
    assert!(
        result.is_failure(),
        "removed key execution should fail: {:?}",
        result.failures(),
    );

    assert!(uac.get_key(removed_key).await.is_none());
    let retained_entry = uac.get_key(retained_key).await.unwrap();
    assert_eq!(
        retained_entry.nonce.0, 0,
        "failed removed-key execution must not affect retained key state",
    );
    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 0, "removed key payload must not execute");
}
