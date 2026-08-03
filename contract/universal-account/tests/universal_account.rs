#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use anyhow::Result;
use common::{
    add_key, create_account, deploy_with_init, execute_as, ft_balance_of, ft_id,
    ft_storage_deposit, get_counter, get_key, harness, list_keys, mint_action, remove_key,
    test_signer, to_sdk, ua_id,
};
use near_api::{AccountId, Signer};
use near_sdk::{
    borsh, env,
    json_types::{U128, U64},
    Gas,
};
use near_token::NearToken;
use rstest::rstest;
use std::sync::Arc;
use templar_gateway_testing::SandboxHarness;
use templar_universal_account::{
    authentication::{with_raw_string::WithRawString, Payload},
    transaction::{FunctionCallAction, Transaction},
    ExecuteArgs, KeyParameters, PayloadExecutionParameters, NEAR_TESTNET_CHAIN_ID,
};
use test_utils::test_signer::TestSigner;

struct Setup {
    ua: AccountId,
    ft: AccountId,
    relayer: AccountId,
    relayer_signer: Arc<Signer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ExecuteOnCreate {
    None,
    Empty,
    Counter,
}

async fn setup(
    harness: &SandboxHarness,
    sk: &TestSigner,
    execute_on_create: ExecuteOnCreate,
) -> Result<Setup> {
    let network = &harness.network;
    let ua = ua_id(harness);
    let ft = ft_id(harness);

    let (relayer, relayer_signer) = create_account(harness, "relayer").await?;

    let execute = match execute_on_create {
        ExecuteOnCreate::None => None,
        ExecuteOnCreate::Empty => Some(vec![]),
        ExecuteOnCreate::Counter => Some(vec![Transaction {
            receiver_id: to_sdk(&ft),
            actions: vec![FunctionCallAction::new(
                "increment",
                b"{}",
                NearToken::from_near(0),
                Gas::from_tgas(3),
            )
            .into()]
            .into(),
        }]),
    };

    deploy_with_init(
        network,
        &ua,
        test_signer(),
        templar_gateway_testing::wasm::universal_account()
            .await
            .to_vec(),
        "new",
        serde_json::json!({
            "key": sk.id(),
            "chain_id": U128(NEAR_TESTNET_CHAIN_ID),
            "execute": execute,
        }),
    )
    .await?;

    let counter = get_counter(network, &ft, &ua).await?;
    if execute_on_create == ExecuteOnCreate::Counter {
        assert_eq!(counter, 1);
    } else {
        assert_eq!(counter, 0);
    }

    ft_storage_deposit(network, &ft, &ua, &relayer, relayer_signer.clone()).await?;

    Ok(Setup {
        ua,
        ft,
        relayer,
        relayer_signer,
    })
}

fn signed_mint_execute_args(
    sk: &TestSigner,
    ft: &AccountId,
    parameters: PayloadExecutionParameters,
    amount: u128,
) -> ExecuteArgs<Box<[Transaction]>> {
    let payload = WithRawString::from_parsed(Payload::new(
        parameters,
        vec![Transaction {
            receiver_id: to_sdk(ft),
            actions: vec![mint_action(amount).into()].into(),
        }]
        .into(),
    ));

    sk.execute_args(payload)
}

/// One account per supported signing format, initialized fresh through the
/// deployed `new` ABI: this proves each `KeyId` variant deserializes, persists,
/// lists, and executes on-chain, and that nonces advance under nearcore's real
/// host crypto. Per-format signature verification and every execution-parameter
/// mismatch are proven exhaustively and off-node by `ExecuteArgs::verify`'s
/// table in `universal-account/src/execute_args.rs`.
#[rstest]
#[tokio::test]
async fn execute_advances_nonce(
    #[future(awt)] harness: SandboxHarness,
    #[values(
        TestSigner::random_passkey(),
        TestSigner::random_ed25519_raw(),
        TestSigner::random_eip191(),
        TestSigner::random_eip712(),
        TestSigner::random_sep53()
    )]
    sk: TestSigner,
) -> Result<()> {
    let Setup {
        ua,
        ft,
        relayer,
        relayer_signer,
    } = setup(&harness, &sk, ExecuteOnCreate::None).await?;
    let network = &harness.network;

    let key_list = list_keys(network, &ua).await?;
    assert_eq!(
        key_list,
        vec![sk.id()],
        "Key should be the only one in control of the account immediately after deployment"
    );

    let key_entry = get_key(network, &ua, &sk.id()).await?.unwrap();
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
            .verifying_contract(to_sdk(&ua))
            .build_salt(),
        vec![Transaction {
            receiver_id: to_sdk(&ft),
            actions: vec![mint_action(100).into()].into(),
        }]
        .into(),
    ));

    execute_as(
        network,
        &ua,
        &relayer,
        relayer_signer.clone(),
        sk.execute_args(payload),
    )
    .await?
    .assert_success();

    assert_eq!(
        ft_balance_of(network, &ft, &ua).await?,
        100,
        "Function call should succeed"
    );

    let key_entry = get_key(network, &ua, &sk.id()).await?.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 1);
    assert_eq!(key_entry.chain_id, Some(NEAR_TESTNET_CHAIN_ID.into()));
    assert_eq!(key_entry.name, Some("Templar Universal Account".into()));
    assert_eq!(key_entry.verifying_contract, to_sdk(&ua));
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

    // Second execution, check nonce advancement.

    let payload = WithRawString::from_parsed(Payload::new(
        key_entry.next_nonce(),
        vec![Transaction {
            receiver_id: to_sdk(&ft),
            actions: vec![mint_action(100).into()].into(),
        }]
        .into(),
    ));

    execute_as(
        network,
        &ua,
        &relayer,
        relayer_signer,
        sk.execute_args(payload),
    )
    .await?
    .assert_success();

    assert_eq!(
        ft_balance_of(network, &ft, &ua).await?,
        200,
        "Function call should succeed"
    );

    let key_entry = get_key(network, &ua, &sk.id()).await?.unwrap();

    assert_eq!(key_entry.block_height, block_height);
    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 2);

    Ok(())
}

/// The `execute` init arg runs constructor transactions atomically with `new`.
/// `Counter` asserts the transaction ran (verified in `setup`); `Empty` asserts
/// an empty batch is a no-op. Either way, `new` must still install the controller
/// key (not skip it because `execute` is `Some`), and the constructor batch must
/// not consume the key's nonce.
#[rstest]
#[tokio::test]
async fn execute_on_create_runs_constructor_transactions(
    #[future(awt)] harness: SandboxHarness,
    #[values(ExecuteOnCreate::Empty, ExecuteOnCreate::Counter)] execute_on_create: ExecuteOnCreate,
) -> Result<()> {
    let sk = TestSigner::random_passkey();
    let Setup { ua, .. } = setup(&harness, &sk, execute_on_create).await?;
    let network = &harness.network;

    assert_eq!(
        list_keys(network, &ua).await?,
        vec![sk.id()],
        "new must install the controller key alongside the constructor batch",
    );
    assert_eq!(
        get_key(network, &ua, &sk.id()).await?.unwrap().nonce.0,
        0,
        "the constructor batch must not consume the controller key's nonce",
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn failed_execute_does_not_consume_nonce_and_success_consumes_once(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let sk = TestSigner::random_passkey();
    let Setup {
        ua,
        ft,
        relayer,
        relayer_signer,
    } = setup(&harness, &sk, ExecuteOnCreate::None).await?;
    let network = &harness.network;

    let key_entry = get_key(network, &ua, &sk.id()).await?.unwrap();
    assert_eq!(key_entry.nonce.0, 0);

    // Sign a payload with a skipped nonce (2 rather than the expected 1); the
    // pre-verification nonce increment must roll back when verification fails.
    let mut skipped_nonce = key_entry.clone();
    skipped_nonce.nonce = U64(2);
    let execute_args = signed_mint_execute_args(&sk, &ft, skipped_nonce, 100);

    let outcome = execute_as(network, &ua, &relayer, relayer_signer.clone(), execute_args).await?;
    assert!(
        !outcome.success,
        "skipped nonce execution should fail: {}",
        outcome.failures,
    );

    let key_entry_after_failure = get_key(network, &ua, &sk.id()).await?.unwrap();
    assert_eq!(
        key_entry_after_failure.nonce.0, 0,
        "failed verification must roll back the pre-verification nonce increment",
    );
    assert_eq!(
        ft_balance_of(network, &ft, &ua).await?,
        0,
        "failed execution should not mint"
    );

    let execute_args =
        signed_mint_execute_args(&sk, &ft, key_entry_after_failure.next_nonce(), 100);
    execute_as(network, &ua, &relayer, relayer_signer, execute_args)
        .await?
        .assert_success();

    let key_entry_after_success = get_key(network, &ua, &sk.id()).await?.unwrap();
    assert_eq!(
        key_entry_after_success.nonce.0, 1,
        "successful execution must consume exactly one nonce",
    );
    assert_eq!(
        ft_balance_of(network, &ft, &ua).await?,
        100,
        "successful execution should mint once"
    );

    Ok(())
}

#[rstest]
#[tokio::test]
async fn replayed_nonce_fails_without_reexecuting_payload(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let sk = TestSigner::random_passkey();
    let Setup {
        ua,
        ft,
        relayer,
        relayer_signer,
    } = setup(&harness, &sk, ExecuteOnCreate::None).await?;
    let network = &harness.network;

    let key_entry = get_key(network, &ua, &sk.id()).await?.unwrap();
    let execute_args = signed_mint_execute_args(&sk, &ft, key_entry.next_nonce(), 100);

    execute_as(
        network,
        &ua,
        &relayer,
        relayer_signer.clone(),
        execute_args.clone(),
    )
    .await?
    .assert_success();

    let key_entry_after_success = get_key(network, &ua, &sk.id()).await?.unwrap();
    assert_eq!(
        key_entry_after_success.nonce.0, 1,
        "successful execution must consume the signed nonce",
    );
    assert_eq!(
        ft_balance_of(network, &ft, &ua).await?,
        100,
        "payload should execute once"
    );

    // Replay the identical signed args: the consumed nonce must reject it.
    let outcome = execute_as(network, &ua, &relayer, relayer_signer, execute_args).await?;
    assert!(
        !outcome.success,
        "replayed nonce execution should fail: {}",
        outcome.failures,
    );

    let key_entry_after_replay = get_key(network, &ua, &sk.id()).await?.unwrap();
    assert_eq!(
        key_entry_after_replay.nonce.0, 1,
        "failed replay must not advance the nonce",
    );
    assert_eq!(
        ft_balance_of(network, &ft, &ua).await?,
        100,
        "replayed payload must not execute again"
    );

    Ok(())
}

#[rstest]
#[tokio::test]
async fn key_indexes_are_unique_across_remove_and_readd(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let sk1 = TestSigner::random_passkey();
    let sk2 = TestSigner::random_ed25519_raw();
    let Setup { ua, .. } = setup(&harness, &sk1, ExecuteOnCreate::None).await?;
    let network = &harness.network;

    let key1 = sk1.id();
    let key2 = sk2.id();

    let initial_entry = get_key(network, &ua, &key1).await?.unwrap();
    assert_eq!(initial_entry.index.0, 0);

    add_key(network, &ua, &key2).await?.assert_success();
    let second_entry = get_key(network, &ua, &key2).await?.unwrap();
    assert_eq!(second_entry.index.0, 1);

    remove_key(network, &ua, &key1).await?.assert_success();
    assert!(get_key(network, &ua, &key1).await?.is_none());

    add_key(network, &ua, &key1).await?.assert_success();
    let readded_entry = get_key(network, &ua, &key1).await?.unwrap();
    assert_eq!(
        readded_entry.index.0, 2,
        "re-added keys must receive a fresh monotonic index",
    );
    assert_eq!(readded_entry.nonce.0, 0);

    let listed_keys = list_keys(network, &ua).await?;
    assert_eq!(listed_keys.len(), 2);
    assert!(listed_keys.contains(&key1));
    assert!(listed_keys.contains(&key2));

    Ok(())
}

#[rstest]
#[tokio::test]
async fn cannot_remove_last_key(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let sk = TestSigner::random_passkey();
    let Setup { ua, .. } = setup(&harness, &sk, ExecuteOnCreate::None).await?;
    let network = &harness.network;

    remove_key(network, &ua, &sk.id())
        .await?
        .assert_failure_contains("Cannot remove last key");

    let keys = list_keys(network, &ua).await?;
    assert_eq!(keys, vec![sk.id()]);
    assert!(get_key(network, &ua, &sk.id()).await?.is_some());

    Ok(())
}

#[rstest]
#[tokio::test]
async fn removed_key_cannot_execute_transaction(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let removed_sk = TestSigner::random_passkey();
    let retained_sk = TestSigner::random_ed25519_raw();
    let Setup {
        ua,
        ft,
        relayer,
        relayer_signer,
    } = setup(&harness, &removed_sk, ExecuteOnCreate::None).await?;
    let network = &harness.network;

    let removed_key = removed_sk.id();
    let retained_key = retained_sk.id();

    add_key(network, &ua, &retained_key).await?.assert_success();

    let removed_entry_before = get_key(network, &ua, &removed_key).await?.unwrap();
    remove_key(network, &ua, &removed_key)
        .await?
        .assert_success();
    assert!(get_key(network, &ua, &removed_key).await?.is_none());

    let execute_args =
        signed_mint_execute_args(&removed_sk, &ft, removed_entry_before.next_nonce(), 100);
    let outcome = execute_as(network, &ua, &relayer, relayer_signer, execute_args).await?;
    assert!(
        !outcome.success,
        "removed key execution should fail: {}",
        outcome.failures,
    );

    assert!(get_key(network, &ua, &removed_key).await?.is_none());
    let retained_entry = get_key(network, &ua, &retained_key).await?.unwrap();
    assert_eq!(
        retained_entry.nonce.0, 0,
        "failed removed-key execution must not affect retained key state",
    );
    assert_eq!(
        ft_balance_of(network, &ft, &ua).await?,
        0,
        "removed key payload must not execute"
    );

    Ok(())
}
