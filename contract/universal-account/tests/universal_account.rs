#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use anyhow::Result;
use common::{
    create_account, deploy_code, deploy_with_init, execute_as, ft_balance_of, ft_id,
    ft_storage_deposit, get_counter, get_key, harness, list_keys, migrate, mint_action,
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
    state,
    transaction::{FunctionCallAction, Transaction},
    InitArgs, KeyParameters, PayloadExecutionParameters, NEAR_TESTNET_CHAIN_ID,
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
    migrated: bool,
    execute_on_create: ExecuteOnCreate,
) -> Result<Setup> {
    let network = &harness.network;
    let ua = ua_id(harness);
    let ft = ft_id(harness);

    let relayer: AccountId = "relayer.near".parse()?;
    let relayer_signer = create_account(harness, &relayer, NearToken::from_near(50)).await?;

    if migrated {
        deploy_with_init(
            network,
            &ua,
            test_signer(),
            test_utils::UniversalAccountController::wasm_0_2_0().to_vec(),
            "new",
            serde_json::json!({ "key": sk.id() }),
        )
        .await?;

        deploy_code(
            network,
            &ua,
            test_signer(),
            test_utils::UniversalAccountController::wasm()
                .await
                .to_vec(),
        )
        .await?;

        migrate(
            network,
            &ua,
            state::Migration::from(state::migration::V0 {
                chain_id: U128(NEAR_TESTNET_CHAIN_ID),
            }),
        )
        .await?
        .assert_success();

        migrate(network, &ua, state::Migration::from(state::migration::V1))
            .await?
            .assert_success();
    } else {
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
            test_utils::UniversalAccountController::wasm()
                .await
                .to_vec(),
            "new",
            InitArgs {
                key: sk.id(),
                chain_id: NEAR_TESTNET_CHAIN_ID.into(),
                execute,
            },
        )
        .await?;
    }

    let counter = get_counter(network, &ft, &ua).await?;
    if execute_on_create == ExecuteOnCreate::Counter && !migrated {
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

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn universal_account(
    #[future(awt)] harness: SandboxHarness,
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
) -> Result<()> {
    let Setup {
        ua,
        ft,
        relayer,
        relayer_signer,
    } = setup(&harness, &sk, migrated, execute_on_create).await?;
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

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn skip_nonce(
    #[future(awt)] harness: SandboxHarness,
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
) -> Result<()> {
    let Setup {
        ua,
        ft,
        relayer,
        relayer_signer,
    } = setup(&harness, &sk, migrated, execute_on_create).await?;
    let network = &harness.network;

    let key_entry = get_key(network, &ua, &sk.id()).await?.unwrap();
    let block_height = key_entry.block_height;

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

    // Try to skip a nonce.

    let payload = WithRawString::from_parsed(Payload::new(
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .with_key_parameters(KeyParameters {
                block_height,
                index: U64(0),
                nonce: U64(3),
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
        relayer_signer,
        sk.execute_args(payload),
    )
    .await?
    .assert_failure_contains(
        "Smart contract panicked: Execution parameter `nonce` mismatch: expected `2`, got `3`",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn reuse_nonce(
    #[future(awt)] harness: SandboxHarness,
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
) -> Result<()> {
    let Setup {
        ua,
        ft,
        relayer,
        relayer_signer,
    } = setup(&harness, &sk, migrated, execute_on_create).await?;
    let network = &harness.network;

    let key_entry = get_key(network, &ua, &sk.id()).await?.unwrap();
    let block_height = key_entry.block_height;

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

    // Try to reuse a nonce.

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
        relayer_signer,
        sk.execute_args(payload),
    )
    .await?
    .assert_failure_contains(
        "Smart contract panicked: Execution parameter `nonce` mismatch: expected `2`, got `1`",
    );

    Ok(())
}
