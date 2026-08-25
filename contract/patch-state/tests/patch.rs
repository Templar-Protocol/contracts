use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use near_api::{types::AccountId, Contract};
use templar_gateway_methods_spec::{account, tx};
use templar_gateway_testing::{wasm, SandboxHarness};
use templar_gateway_types::{
    common::{ContractArgs, WriteOperationResult},
    ActionInput, Base64Bytes, ContractMethodName, CryptoHash as GatewayCryptoHash,
    GlobalContractIdentifierInput, ManagedAccountId, NearGas, NearToken, OperationStatus,
};
use templar_patch_state_types::{Op, Patch};

const PATCH_GAS: NearGas = NearGas::from_tgas(300);

fn assert_failure(result: &WriteOperationResult, message: &str) {
    assert_eq!(result.operation.status, OperationStatus::Failed);
    assert!(
        result
            .operation
            .failure_message()
            .unwrap_or_default()
            .contains(message),
        "unexpected failure reason: {:?}",
        result.operation.failure_message()
    );
}

fn patch_call(patch: &Patch) -> Result<ActionInput> {
    Ok(ActionInput::FunctionCall {
        method_name: ContractMethodName("patch".to_owned()),
        args: ContractArgs::Raw(Base64Bytes(borsh::to_vec(patch)?)),
        gas: PATCH_GAS,
        deposit: NearToken::ZERO,
    })
}

fn deploy(code: Vec<u8>) -> ActionInput {
    ActionInput::DeployContract {
        code: Base64Bytes(code),
    }
}

fn patch_for(account_id: &AccountId, ops: Vec<Op>) -> Patch {
    Patch {
        account_id: account_id.to_string(),
        ops,
    }
}

async fn storage_value(
    harness: &SandboxHarness,
    account_id: &AccountId,
    key: &[u8],
) -> Result<Option<Vec<u8>>> {
    let storage = Contract(account_id.clone())
        .view_storage()
        .fetch_from(&harness.network)
        .await?
        .data;

    for entry in storage.values {
        if STANDARD.decode(entry.key.0)? == key {
            return Ok(Some(STANDARD.decode(entry.value.0)?));
        }
    }

    Ok(None)
}

async fn local_market_target(
    harness: &SandboxHarness,
    label: &str,
) -> Result<(ManagedAccountId, Vec<u8>, String)> {
    let target = harness.create_user(label).await?;
    let original = wasm::market().await.to_vec();
    harness.deploy_code(&target.0, original.clone()).await?;
    let code_hash = harness.code_hash(&target.0).await?;
    Ok((target, original, code_hash))
}

#[tokio::test]
async fn patch_batch_restores_market_code_and_applies_storage() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let (target, original, original_code_hash) =
        local_market_target(&harness, "patch-target").await?;
    let patch_wasm = wasm::patch_state().await.to_vec();
    assert!(
        patch_wasm.len() < 16 * 1024,
        "patch WASM is {} bytes",
        patch_wasm.len()
    );
    assert!(
        original.len() >= 500_000,
        "market fixture is only {} bytes",
        original.len()
    );

    harness
        .patch_state(&target.0, [(b"stale".to_vec(), b"before".to_vec())])
        .await?;

    let result = harness
        .execute(
            &target,
            tx::Batch {
                receiver_id: target.0.clone(),
                actions: vec![
                    deploy(patch_wasm),
                    patch_call(&patch_for(
                        &target.0,
                        vec![
                            Op::Expect {
                                key: b"stale".to_vec(),
                                value: Some(b"before".to_vec()),
                            },
                            Op::Set {
                                key: b"fresh".to_vec(),
                                value: b"after".to_vec(),
                            },
                            Op::Remove {
                                key: b"stale".to_vec(),
                            },
                        ],
                    ))?,
                    deploy(original),
                ],
            },
        )
        .await?;

    assert_eq!(result.operation.steps.len(), 1);
    assert_eq!(harness.code_hash(&target.0).await?, original_code_hash);
    assert_eq!(storage_value(&harness, &target.0, b"stale").await?, None);
    assert_eq!(
        storage_value(&harness, &target.0, b"fresh").await?,
        Some(b"after".to_vec())
    );

    let gas_burnt = harness.operation_gas_burnt(&result);
    assert!(gas_burnt > 0);
    eprintln!(
        "patch market batch: market_wasm_bytes={}, patch_wasm_bytes={}, total_gas_burnt={gas_burnt}",
        wasm::market().await.len(),
        wasm::patch_state().await.len(),
    );

    Ok(())
}

#[tokio::test]
async fn expect_mismatch_reverts_code_and_storage() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let (target, original, original_code_hash) =
        local_market_target(&harness, "expect-mismatch").await?;
    harness
        .patch_state(&target.0, [(b"guard".to_vec(), b"actual".to_vec())])
        .await?;

    let result = harness
        .try_execute(
            &target,
            tx::Batch {
                receiver_id: target.0.clone(),
                actions: vec![
                    deploy(wasm::patch_state().await.to_vec()),
                    patch_call(&patch_for(
                        &target.0,
                        vec![Op::Expect {
                            key: b"guard".to_vec(),
                            value: Some(b"expected".to_vec()),
                        }],
                    ))?,
                    deploy(original),
                ],
            },
        )
        .await?;

    assert_failure(&result, "storage expectation failed");
    assert_eq!(harness.code_hash(&target.0).await?, original_code_hash);
    assert_eq!(
        storage_value(&harness, &target.0, b"guard").await?,
        Some(b"actual".to_vec())
    );
    Ok(())
}

#[tokio::test]
async fn expect_hash_mismatch_reverts_code_and_storage() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let (target, original, original_code_hash) =
        local_market_target(&harness, "hash-mismatch").await?;
    harness
        .patch_state(&target.0, [(b"guard".to_vec(), b"actual".to_vec())])
        .await?;
    let mut wrong_hash = near_api::types::CryptoHash::hash(b"actual").0;
    wrong_hash[0] ^= 1;

    let result = harness
        .try_execute(
            &target,
            tx::Batch {
                receiver_id: target.0.clone(),
                actions: vec![
                    deploy(wasm::patch_state().await.to_vec()),
                    patch_call(&patch_for(
                        &target.0,
                        vec![Op::ExpectHash {
                            key: b"guard".to_vec(),
                            sha256: wrong_hash,
                        }],
                    ))?,
                    deploy(original),
                ],
            },
        )
        .await?;

    assert_failure(&result, "storage hash expectation failed");
    assert_eq!(harness.code_hash(&target.0).await?, original_code_hash);
    assert_eq!(
        storage_value(&harness, &target.0, b"guard").await?,
        Some(b"actual".to_vec())
    );
    Ok(())
}

#[tokio::test]
async fn patch_rejects_foreign_predecessor_and_wrong_target() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let target = harness.create_user("patch-access").await?;
    harness
        .deploy_code(&target.0, wasm::patch_state().await.to_vec())
        .await?;

    let foreign = harness
        .try_execute(
            &harness.gateway_signer_account_id,
            tx::FunctionCall {
                receiver_id: target.0.clone(),
                method_name: ContractMethodName("patch".to_owned()),
                args: ContractArgs::Raw(Base64Bytes(borsh::to_vec(&patch_for(&target.0, vec![]))?)),
                gas: PATCH_GAS,
                deposit: NearToken::ZERO,
            },
        )
        .await?;
    assert_failure(&foreign, "patch must be called by the target account");

    let wrong_target = harness
        .try_execute(
            &target,
            tx::FunctionCall {
                receiver_id: target.0.clone(),
                method_name: ContractMethodName("patch".to_owned()),
                args: ContractArgs::Raw(Base64Bytes(borsh::to_vec(&Patch {
                    account_id: "other.near".to_owned(),
                    ops: vec![],
                })?)),
                gas: PATCH_GAS,
                deposit: NearToken::ZERO,
            },
        )
        .await?;
    assert_failure(&wrong_target, "patch target does not match current account");

    Ok(())
}

#[tokio::test]
async fn patch_batch_restores_global_contract_target() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let target = harness.create_user("global-target").await?;
    let original = wasm::market().await.to_vec();
    let global_hash = harness.deploy_global_contract(original).await?;
    let expected_hash = global_hash.to_string();

    harness
        .execute(
            &target,
            tx::Batch {
                receiver_id: target.0.clone(),
                actions: vec![ActionInput::UseGlobalContract {
                    contract_identifier: GlobalContractIdentifierInput::CodeHash(
                        GatewayCryptoHash(global_hash),
                    ),
                }],
            },
        )
        .await?;
    let before = harness
        .client()?
        .read(account::Get {
            account_id: target.0.clone(),
        })
        .await?;
    assert_eq!(
        before.global_contract_hash.as_deref(),
        Some(expected_hash.as_str())
    );

    harness
        .execute(
            &target,
            tx::Batch {
                receiver_id: target.0.clone(),
                actions: vec![
                    deploy(wasm::patch_state().await.to_vec()),
                    patch_call(&patch_for(
                        &target.0,
                        vec![Op::Set {
                            key: b"global".to_vec(),
                            value: b"restored".to_vec(),
                        }],
                    ))?,
                    ActionInput::UseGlobalContract {
                        contract_identifier: GlobalContractIdentifierInput::CodeHash(
                            GatewayCryptoHash(global_hash),
                        ),
                    },
                ],
            },
        )
        .await?;

    let after = harness
        .client()?
        .read(account::Get {
            account_id: target.0.clone(),
        })
        .await?;
    assert_eq!(
        after.global_contract_hash.as_deref(),
        Some(expected_hash.as_str())
    );
    assert_eq!(
        storage_value(&harness, &target.0, b"global").await?,
        Some(b"restored".to_vec())
    );

    Ok(())
}
