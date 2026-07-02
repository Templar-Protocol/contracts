use super::*;

#[tokio::test]
async fn tx_function_call_and_view_function_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;

    let _ = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: tx::FunctionCall {
                receiver_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("set_redemption_rate".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "redemption_rate": NearToken::from_near(2).as_yoctonear().to_string(),
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        })
        .await?;

    let counter = stack
        .controller
        .request::<contract::ViewFunction>(&contract::ViewFunction {
            contract_id: stack.harness.ft_contract_id.clone(),
            method_name: ContractMethodName("redemption_rate".to_owned()),
            args: ContractArgs::Raw(Base64Bytes(Vec::new())),
        })
        .await?;

    assert_eq!(
        counter.value,
        serde_json::json!(NearToken::from_near(2).as_yoctonear().to_string())
    );

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn tx_function_call_idempotency_reuses_the_same_operation() -> Result<()> {
    let stack = TestStack::start().await?;

    let first = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: Some(templar_gateway_types::IdempotencyKey(
                "set-redemption-rate".to_owned(),
            )),
            body: tx::FunctionCall {
                receiver_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("set_redemption_rate".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "redemption_rate": NearToken::from_near(3).as_yoctonear().to_string(),
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        })
        .await?;
    let second = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: Some(templar_gateway_types::IdempotencyKey(
                "set-redemption-rate".to_owned(),
            )),
            body: tx::FunctionCall {
                receiver_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("set_redemption_rate".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "redemption_rate": NearToken::from_near(3).as_yoctonear().to_string(),
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        })
        .await?;

    assert_eq!(first.operation.id, second.operation.id);
    assert_eq!(first.operation.steps, second.operation.steps);

    let fetched = stack
        .controller
        .request::<op::Get>(&op::Get {
            operation_id: first.operation.id.clone(),
        })
        .await?;

    assert_eq!(fetched.operation, Some(first.operation.clone()));

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn tx_transfer_unregister_and_account_delete_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;

    let _ = register_gateway_signer_for_ft(&stack).await?;

    let _ = stack
        .controller
        .request::<storage::Deposit>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: storage::Deposit {
                contract_id: stack.harness.ft_contract_id.clone(),
                beneficiary_id: Some(stack.harness.beneficiary_account_id.clone()),
                registration_only: false,
                deposit: NearToken::from_near(1),
            },
        })
        .await?;

    let _ = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: tx::FunctionCall {
                receiver_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("mint".to_owned()),
                args: ContractArgs::Json(serde_json::json!({ "amount": "3" })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        })
        .await?;

    let _ = stack
        .controller
        .request::<ft::Transfer>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: ft::Transfer {
                contract_id: stack.harness.ft_contract_id.clone(),
                receiver_id: stack.harness.beneficiary_account_id.clone(),
                amount: templar_common::SU128::from(3),
                memo: None,
            },
        })
        .await?;

    let balance = stack
        .controller
        .request::<ft::GetBalanceOf>(&ft::GetBalanceOf {
            contract_id: stack.harness.ft_contract_id.clone(),
            account_id: stack.harness.gateway_signer_account_id.0.clone(),
        })
        .await?;

    assert_eq!(balance.balance, templar_common::SU128::from(0));

    let _ = stack
        .controller
        .request::<storage::Unregister>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: storage::Unregister {
                contract_id: stack.harness.ft_contract_id.clone(),
                force: false,
            },
        })
        .await?;

    let storage_balance = stack
        .controller
        .request::<storage::GetBalanceOf>(&storage::GetBalanceOf {
            contract_id: stack.harness.ft_contract_id.clone(),
            account_id: stack.harness.gateway_signer_account_id.0.clone(),
        })
        .await?;

    assert!(storage_balance.balance.is_none());

    let _ = stack
        .controller
        .request::<account::Delete>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            body: account::Delete {
                beneficiary_id: stack.harness.beneficiary_account_id.clone(),
            },
        })
        .await?;

    let deleted = stack
        .controller
        .request::<tx::Get>(&tx::Get {
            tx_hash: CryptoHash(
                "11111111111111111111111111111111"
                    .parse()
                    .expect("valid dummy hash"),
            ),
            sender_account_id: stack.harness.cleanup_signer_account_id.0.clone(),
            wait_until: Some(templar_gateway_types::common::TxExecutionStatus::None),
            encoding: tx::ValueEncoding::Json,
        })
        .await;

    assert!(deleted.is_err());

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn tx_transfer_and_deploy_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;

    let before = stack
        .controller
        .request::<account::Get>(&account::Get {
            account_id: stack.harness.beneficiary_account_id.clone(),
        })
        .await?;

    let transfer = stack
        .controller
        .request::<tx::Transfer>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: tx::Transfer {
                receiver_id: stack.harness.beneficiary_account_id.clone(),
                amount: NearToken::from_yoctonear(1),
            },
        })
        .await?;
    assert_eq!(
        transfer.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    let after = stack
        .controller
        .request::<account::Get>(&account::Get {
            account_id: stack.harness.beneficiary_account_id.clone(),
        })
        .await?;
    assert!(after.amount > before.amount);

    let deploy = stack
        .controller
        .request::<tx::DeployContract>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            body: tx::DeployContract {
                account_id: stack.harness.cleanup_signer_account_id.0.clone(),
                code: Base64Bytes(stack.harness.ft_wasm().await),
            },
        })
        .await?;
    assert_eq!(
        deploy.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    let cleanup_account = stack
        .controller
        .request::<account::Get>(&account::Get {
            account_id: stack.harness.cleanup_signer_account_id.0.clone(),
        })
        .await?;
    assert_ne!(
        cleanup_account.code_hash,
        "11111111111111111111111111111111"
    );

    let deploy_and_init = stack
        .controller
        .request::<tx::DeployAndInit>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: tx::DeployAndInit {
                account_id: stack.harness.proxy_oracle_signer_account_id.0.clone(),
                code: Base64Bytes(stack.harness.ft_wasm().await),
                method_name: ContractMethodName("new".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "name": "Gateway FT",
                    "symbol": "GFT",
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::ZERO,
            },
        })
        .await?;
    assert_eq!(
        deploy_and_init.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    let redemption_rate = view_contract_json(
        &stack,
        stack.harness.proxy_oracle_signer_account_id.0.clone(),
        "redemption_rate",
        serde_json::Value::Null,
    )
    .await?;
    assert_eq!(
        redemption_rate,
        serde_json::json!(NearToken::from_near(1).as_yoctonear().to_string())
    );

    stack.shutdown().await;
    Ok(())
}
