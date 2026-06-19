use near_account_id::AccountId;
use templar_gateway_types::OperationStatus;
use templar_universal_account::PayloadExecutionParameters;
use test_utils::UniversalAccountController;

use super::*;

#[tokio::test]
async fn universal_account_get_key_endpoint_works_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let (account_id, signer) = stack.harness.deploy_universal_account().await?;

    let result = stack
        .controller
        .request::<universal_account::GetKey>(&ReadRequest {
            params: universal_account::GetKey {
                account_id,
                key: signer.id(),
            },
        })
        .await?;

    assert!(result.parameters.is_some());

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn universal_account_write_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let (account_id, signer) = stack.harness.deploy_universal_account().await?;

    let key = stack
        .controller
        .request::<universal_account::GetKey>(&ReadRequest {
            params: universal_account::GetKey {
                account_id: account_id.clone(),
                key: signer.id(),
            },
        })
        .await?
        .parameters
        .expect("deployed universal account should expose its key parameters");

    let payload = WithRawString::from_parsed(Payload::new(
        PayloadExecutionParameters::builder(NEAR_TESTNET_CHAIN_ID)
            .with_key_parameters(KeyParameters {
                block_height: key.block_height.into(),
                index: key.index.into(),
                nonce: (key.nonce + 1).into(),
            })
            .verifying_contract(account_id.clone())
            .build_salt(),
        vec![Transaction {
            receiver_id: stack.harness.ft_contract_id.clone(),
            actions: vec![FunctionCallAction::new(
                "increment",
                b"{}",
                NearToken::from_near(0),
                near_sdk::Gas::from_tgas(3),
            )
            .into()]
            .into(),
        }]
        .into(),
    ));

    let _ = stack
        .controller
        .request::<universal_account::Execute>(&WriteRequest {
            signer_account_id: stack.harness.universal_account_signer_account_id.clone(),
            idempotency_key: None,
            body: universal_account::Execute {
                account_id: account_id.clone(),
                args: signer.execute_args(payload),
            },
        })
        .await?;

    let counter = stack
        .controller
        .request::<contract::ViewFunction>(&ReadRequest {
            params: contract::ViewFunction {
                contract_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("get_counter".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "account_id": account_id,
                })),
            },
        })
        .await?;

    assert_eq!(counter.value, serde_json::json!(1));

    let registry_id = stack.harness.deploy_registry().await?;
    let _ = stack
        .controller
        .request::<registry::AddVersion>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: registry::AddVersion {
                registry_id: registry_id.clone(),
                version_key: "ua@1.0.0".to_owned(),
                deploy_mode: templar_common::registry::DeployMode::Normal,
                code: Base64Bytes(UniversalAccountController::wasm().await.to_vec()),
                deposit: NearToken::from_yoctonear(1),
            },
        })
        .await?;

    let create = stack
        .controller
        .request::<universal_account::Create>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: universal_account::Create {
                registry_id: registry_id.clone(),
                account_name: "ua-created".to_owned(),
                version_key: "ua@1.0.0".to_owned(),
                key: signer.id(),
                chain_id: templar_gateway_types::U128(NEAR_TESTNET_CHAIN_ID),
                execute: None,
                full_access_keys: None,
                deposit: NearToken::from_near(20),
            },
        })
        .await?;

    assert_eq!(create.operation.status, OperationStatus::Succeeded);

    let created_account_id: AccountId = format!("ua-created.{registry_id}")
        .parse()
        .expect("created universal account id should be valid");

    let created_key = stack
        .controller
        .request::<universal_account::GetKey>(&ReadRequest {
            params: universal_account::GetKey {
                account_id: created_account_id,
                key: signer.id(),
            },
        })
        .await?;

    assert!(created_key.parameters.is_some());

    stack.shutdown().await;
    Ok(())
}
