use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use near_account_id::AccountId;
use near_api::SecretKey;
use near_crypto::KeyType;
use near_primitives::{
    account::{AccessKey, Account as ChainAccount, AccountContract},
    hash::CryptoHash as ChainCryptoHash,
    state_record::StateRecord,
};
use near_token::NearToken;
use sha2::{Digest as _, Sha256};

use crate::spec::{
    check::Status,
    patch::{BorshExpr, ByteExpr, PatchSpec, ResolvedExpectation, ResolvedOperation, Sha256Digest},
    patch_plan::RestoreCode,
};
use templar_gateway_types::{ActionInput, CryptoHash};

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("fixtures/spec/patch/{path}"))
}

#[test]
fn byte_expressions_compose_near_collection_keys() {
    let account: AccountId = "alice.near".parse().expect("valid account");
    let expression = ByteExpr::Concat(vec![
        ByteExpr::Hex("00".to_owned()),
        ByteExpr::Sha256(Box::new(ByteExpr::Borsh(BorshExpr {
            type_name: "AccountId".to_owned(),
            value: serde_json::Value::String(account.to_string()),
        }))),
    ]);

    let bytes = expression
        .resolve(std::path::Path::new("."))
        .expect("expression resolves");
    assert_eq!(bytes[0], 0);
    let expected = Sha256::digest(borsh::to_vec(&account).unwrap());
    assert_eq!(&bytes[1..], &expected[..]);
}

#[test]
fn borsh_codecs_are_production_type_encodings() {
    let expression = ByteExpr::Borsh(BorshExpr {
        type_name: "u32".to_owned(),
        value: serde_json::Value::from(7),
    });
    assert_eq!(
        expression.resolve(std::path::Path::new(".")).unwrap(),
        borsh::to_vec(&7u32).unwrap(),
    );

    let account: AccountId = "alice.near".parse().unwrap();
    let cases = [
        (
            "AccountId",
            serde_json::Value::String(account.to_string()),
            borsh::to_vec(&account).unwrap(),
        ),
        (
            "String",
            serde_json::Value::String("hello".to_owned()),
            borsh::to_vec(&"hello".to_owned()).unwrap(),
        ),
        (
            "Vec<u8>",
            serde_json::from_str("[1,2,3]").unwrap(),
            borsh::to_vec(&vec![1_u8, 2, 3]).unwrap(),
        ),
        (
            "bool",
            serde_json::Value::Bool(true),
            borsh::to_vec(&true).unwrap(),
        ),
        (
            "u128",
            serde_json::Value::from(7_u64),
            borsh::to_vec(&7_u128).unwrap(),
        ),
        (
            "u32",
            serde_json::Value::from(7),
            borsh::to_vec(&7_u32).unwrap(),
        ),
        (
            "u64",
            serde_json::Value::from(7_u64),
            borsh::to_vec(&7_u64).unwrap(),
        ),
        (
            "u8",
            serde_json::Value::from(7),
            borsh::to_vec(&7_u8).unwrap(),
        ),
    ];
    for (type_name, value, expected) in cases {
        assert_eq!(
            ByteExpr::Borsh(BorshExpr {
                type_name: type_name.to_owned(),
                value,
            })
            .resolve(std::path::Path::new("."))
            .unwrap(),
            expected
        );
    }
    let unsupported = ByteExpr::Borsh(BorshExpr {
        type_name: "Decimal".to_owned(),
        value: serde_json::Value::String("1.2".to_owned()),
    });
    assert!(unsupported.resolve(std::path::Path::new(".")).is_err());
}

#[test]
fn checked_in_patch_fixture_covers_supported_toml_syntax() {
    let path = fixture("patches/target.near/2026-08-25-syntax.toml");
    let spec = PatchSpec::load(&path).expect("reference spec resolves");
    let resolved = spec.resolve(&path).expect("reference bytes resolve");

    assert_eq!(spec.account_id.as_str(), "target.near");
    assert_eq!(
        resolved
            .operations
            .iter()
            .map(|operation| match operation {
                ResolvedOperation::Expect { .. } => "expect",
                ResolvedOperation::Set { .. } => "set",
                ResolvedOperation::Remove { .. } => "remove",
                ResolvedOperation::RemovePrefix { .. } => "remove_prefix",
            })
            .collect::<Vec<_>>(),
        ["expect", "set", "remove", "remove_prefix", "set", "set"]
    );
    let ResolvedOperation::Set { value, .. } = &resolved.operations[1] else {
        panic!("reference file operation must be a set");
    };
    assert_eq!(value.0, b"reference-position-bytes");
    let ResolvedOperation::Set { value, .. } = &resolved.operations[4] else {
        panic!("reference JSON operation must be a set");
    };
    let expected: serde_json::Value =
        serde_json::from_str(r#"{"enabled":true,"name":"example"}"#).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&value.0).unwrap(),
        expected
    );
    assert!(serde_json::to_value(&resolved)
        .unwrap()
        .to_string()
        .contains(r#""value":"cmVmZXJlbmNlLXBvc2l0aW9uLWJ5dGVz""#));

    let error = PatchSpec::load(&fixture("invalid/unknown-field.toml"))
        .expect_err("unknown top-level key must fail");
    assert!(format!("{error:#}").contains("unknown"));
    let error = PatchSpec::load(&fixture("invalid/tagged-expect.toml"))
        .expect_err("remove_prefix expectation must fail");
    assert!(format!("{error:#}").contains("expect"));
    let error = PatchSpec::load(&fixture("invalid/schema-2.toml"))
        .expect_err("schema 2 requires explicit promotion");
    let rendered = format!("{error:#}");
    assert!(rendered.contains("schema = 3"));
    assert!(rendered.contains("tmplrmgr patch export <target-account> --out <new-schema-3-path>"));

    let ResolvedOperation::Set { expected, .. } = &resolved.operations[5] else {
        panic!("fresh-key operation must be a set");
    };
    assert!(matches!(expected, Some(ResolvedExpectation::Absent)));
}

#[test]
fn sha256_digest_uses_lowercase_hex() {
    let digest = Sha256Digest([0xab; 32]);
    let json = serde_json::to_string(&digest).unwrap();
    assert_eq!(
        json,
        r#""abababababababababababababababababababababababababababababababab""#
    );
    assert_eq!(serde_json::from_str::<Sha256Digest>(&json).unwrap(), digest);
    assert!(serde_json::from_str::<Sha256Digest>(
        r#""ABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB""#
    )
    .is_err());
}

#[test]
fn restore_code_variants_round_trip_as_tagged_json() {
    let account_id: AccountId = "code.near".parse().unwrap();
    let hash = CryptoHash::from(near_api::types::CryptoHash([7; 32]));
    let cases = [
        (
            RestoreCode::Local { code_hash: hash },
            r#"{"mode":"local","code_hash":"US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx"}"#,
        ),
        (
            RestoreCode::GlobalCodeHash { hash },
            r#"{"mode":"global_code_hash","hash":"US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx"}"#,
        ),
        (
            RestoreCode::GlobalAccount { account_id },
            r#"{"mode":"global_account","account_id":"code.near"}"#,
        ),
    ];
    for (restore, expected) in cases {
        let encoded = serde_json::to_value(&restore).unwrap();
        assert_eq!(
            encoded,
            serde_json::from_str::<serde_json::Value>(expected).unwrap()
        );
        assert_eq!(
            serde_json::from_value::<RestoreCode>(encoded).unwrap(),
            restore
        );
    }
}

#[test]
fn proxy_migration_patch_declares_expected_views() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../../deployments/patches/proxy-oracle-ixlmustry-ixlmusdc.v1.tmplr.near/2026-08-27-proxy-oracle-v0-to-v1.toml",
    );
    let spec = PatchSpec::load(&path).unwrap();
    assert_eq!(spec.ops.len(), 10);
    assert_eq!(spec.view_checks().count(), 4);
}

#[allow(
    clippy::too_many_lines,
    reason = "one integration scenario deliberately presents fixture setup, plan, replay, and stamp assertions together"
)]
#[tokio::test]
async fn patch_dry_run_replays_proxy_v0_to_v1() -> Result<()> {
    let harness = templar_gateway_testing::SandboxHarness::start().await?;
    let account_id: AccountId = "proxy-oracle-ixlmustry-ixlmusdc.v1.tmplr.near".parse()?;
    let key: SecretKey = near_crypto::SecretKey::from_random(KeyType::ED25519)
        .to_string()
        .parse()?;
    let public_key: near_crypto::PublicKey = key.public_key().to_string().parse()?;
    let current_code = templar_gateway_testing::wasm::proxy_oracle().await.to_vec();
    let current_hash = CryptoHash::from(near_api::types::CryptoHash::hash(&current_code));
    let old_state: HashMap<Vec<u8>, Vec<u8>> = borsh::from_slice(include_bytes!(
        "../../../../contract/proxy-oracle/near/contract/tests/migration/mainnet_proxy_oracle_ixlmustry_ixlmusdc.borsh"
    ))?;
    let access_key = AccessKey::full_access();
    let barrier_key: SecretKey = near_crypto::SecretKey::from_random(KeyType::ED25519)
        .to_string()
        .parse()?;
    let barrier_public_key: near_crypto::PublicKey =
        barrier_key.public_key().to_string().parse()?;
    let barrier_access_key = AccessKey::full_access();
    let limits = harness
        .client()?
        .read(templar_gateway_methods_spec::chain::GetProtocolLimits)
        .await?;
    let storage_usage = limits.num_bytes_account
        + u64::try_from(current_code.len())?
        + u64::try_from(borsh::to_vec(&public_key)?.len())?
        + u64::try_from(borsh::to_vec(&access_key)?.len())?
        + limits.num_extra_bytes_record
        + old_state
            .iter()
            .map(|(key, value)| {
                u64::try_from(key.len() + value.len()).unwrap() + limits.num_extra_bytes_record
            })
            .sum::<u64>();
    let final_storage_usage = storage_usage
        + u64::try_from(borsh::to_vec(&barrier_public_key)?.len())?
        + u64::try_from(borsh::to_vec(&barrier_access_key)?.len())?
        + limits.num_extra_bytes_record;
    let network = harness.network.clone();
    templar_sandbox::patch_records(
        &network,
        vec![
            StateRecord::Account {
                account_id: account_id.clone(),
                account: ChainAccount::new(
                    NearToken::from_near(100_000_000),
                    NearToken::from_yoctonear(0),
                    AccountContract::Local(ChainCryptoHash(current_hash.0 .0)),
                    storage_usage,
                ),
            },
            StateRecord::AccessKey {
                account_id: account_id.clone(),
                public_key: public_key.clone(),
                access_key: access_key.clone(),
            },
        ],
    )
    .await?;
    let remote = templar_gateway_client::Client::builder(network.clone())
        .secret_key(account_id.clone(), key.clone())?
        .build()?;
    let deploy = remote
        .execute_as(
            account_id.clone(),
            templar_gateway_methods_spec::tx::Batch {
                receiver_id: account_id.clone(),
                actions: vec![ActionInput::DeployContract {
                    code: templar_gateway_types::Base64Bytes(
                        templar_gateway_testing::wasm::released(
                            templar_gateway_testing::ArtifactId::ProxyOracle,
                            "0.1.0",
                        )
                        .await,
                    ),
                }],
            },
        )
        .await?;
    assert_eq!(
        deploy.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );
    templar_sandbox::patch_data(&network, &account_id, old_state.into_iter()).await?;
    let redeploy = remote
        .execute_as(
            account_id.clone(),
            templar_gateway_methods_spec::tx::Batch {
                receiver_id: account_id.clone(),
                actions: vec![ActionInput::DeployContract {
                    code: templar_gateway_types::Base64Bytes(current_code),
                }],
            },
        )
        .await?;
    assert_eq!(
        redeploy.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );
    templar_sandbox::patch_records(
        &network,
        vec![
            StateRecord::Account {
                account_id: account_id.clone(),
                account: ChainAccount::new(
                    NearToken::from_near(100_000_000),
                    NearToken::from_yoctonear(0),
                    AccountContract::Local(ChainCryptoHash(current_hash.0 .0)),
                    final_storage_usage,
                ),
            },
            StateRecord::AccessKey {
                account_id: account_id.clone(),
                public_key: barrier_public_key.clone(),
                access_key: barrier_access_key,
            },
        ],
    )
    .await?;
    templar_sandbox::wait_until_final(&network, &account_id, &barrier_public_key).await?;

    let spec_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../../deployments/patches/proxy-oracle-ixlmustry-ixlmusdc.v1.tmplr.near/2026-08-27-proxy-oracle-v0-to-v1.toml",
    );
    let plan_path = std::env::temp_dir().join(format!(
        "templar-manager-proxy-plan-{}.json",
        std::process::id()
    ));
    let cli = crate::cli::Cli {
        network: templar_gateway_client::Network::Mainnet,
        rpc_url: Some(network.rpc_endpoints[0].url.to_string()),
        rpc_api_key: None,
        transaction_url_prefix: None,
        quiet: 1,
        verbose: 0,
        command: crate::cli::Command::Patch {
            command: crate::commands::patch::PatchNs::Plan(crate::commands::patch::Plan {
                path: spec_path,
                out: Some(plan_path.clone()),
                signer_id: account_id.clone(),
                public_key: key.public_key(),
                skip_check: Vec::new(),
                allow_unguarded: false,
            }),
        },
    };
    let ctx = crate::context::build_context(&cli)?;
    crate::dispatch::dispatch(ctx, cli.command).await?;
    let cli = crate::cli::Cli {
        network: templar_gateway_client::Network::Mainnet,
        rpc_url: Some(network.rpc_endpoints[0].url.to_string()),
        rpc_api_key: None,
        transaction_url_prefix: None,
        quiet: 1,
        verbose: 0,
        command: crate::cli::Command::Patch {
            command: crate::commands::patch::PatchNs::DryRun(crate::commands::patch::DryRun {
                plan: plan_path.clone(),
                skip_check: Vec::new(),
                allow_unguarded: false,
            }),
        },
    };
    let ctx = crate::context::build_context(&cli)?;
    crate::dispatch::dispatch(ctx, cli.command).await?;
    let plan: crate::spec::patch_plan::PatchPlan =
        serde_json::from_str(&std::fs::read_to_string(&plan_path)?)?;
    let stamp = plan
        .dry_run
        .as_ref()
        .expect("successful dry-run is stamped");
    assert_eq!(plan.state_digest, stamp.state_digest);
    assert!(stamp.checks.iter().any(
        |check| check.id == "patch.key_length" && matches!(check.status, Status::Passed { .. })
    ));
    assert!(stamp
        .checks
        .iter()
        .any(|check| check.id == "patch.dry_run" && matches!(check.status, Status::Passed { .. })));
    let _ = std::fs::remove_file(plan_path);
    Ok(())
}
