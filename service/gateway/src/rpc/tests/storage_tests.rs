use super::*;

#[tokio::test]
async fn storage_and_get_transaction_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;

    let _bounds = register_gateway_signer_for_ft(&stack).await?;

    let balance_before = stack
        .controller
        .request::<storage::GetBalanceOf>(&storage::GetBalanceOf {
            contract_id: stack.harness.ft_contract_id.clone(),
            account_id: stack.harness.gateway_signer_account_id.0.clone(),
        })
        .await?;

    assert!(balance_before.balance.is_some());

    let beneficiary_deposit = stack
        .controller
        .request::<storage::Deposit>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: storage::Deposit {
                contract_id: stack.harness.ft_contract_id.clone(),
                beneficiary_id: Some(stack.harness.beneficiary_account_id.clone()),
                registration_only: true,
                deposit: NearToken::from_near(1),
            },
        })
        .await?;

    let beneficiary_balance = stack
        .controller
        .request::<storage::GetBalanceOf>(&storage::GetBalanceOf {
            contract_id: stack.harness.ft_contract_id.clone(),
            account_id: stack.harness.beneficiary_account_id.clone(),
        })
        .await?;

    assert!(beneficiary_balance.balance.is_some());

    let deposit_transaction = stack
        .controller
        .request::<tx::Get>(&tx::Get {
            tx_hash: tx_hash(&beneficiary_deposit),
            sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
            wait_until: None,
            encoding: tx::ValueEncoding::Json,
        })
        .await?;

    assert_eq!(deposit_transaction.status, tx::Status::Succeeded);
    assert!(matches!(
        deposit_transaction.return_value,
        Some(tx::ReturnValue::Json(_))
    ));

    let mint_transaction = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: tx::FunctionCall {
                receiver_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("mint".to_owned()),
                args: ContractArgs::Json(serde_json::json!({ "amount": "1" })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            },
        })
        .await?;

    let mint_status = stack
        .controller
        .request::<tx::Get>(&tx::Get {
            tx_hash: tx_hash(&mint_transaction),
            sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
            wait_until: None,
            encoding: tx::ValueEncoding::Json,
        })
        .await?;

    assert_eq!(mint_status.status, tx::Status::Succeeded);

    let transfer_transaction = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: tx::FunctionCall {
                receiver_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("ft_transfer".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "receiver_id": stack.harness.beneficiary_account_id,
                    "amount": "1",
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(1),
            },
        })
        .await?;

    let transfer_result = stack
        .controller
        .request::<tx::Get>(&tx::Get {
            tx_hash: tx_hash(&transfer_transaction),
            sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
            wait_until: None,
            encoding: tx::ValueEncoding::Json,
        })
        .await?;

    assert_eq!(transfer_result.status, tx::Status::Succeeded);
    assert!(!transfer_result.logs.is_empty());

    let unregister_transaction = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: tx::FunctionCall {
                receiver_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("patch_storage_unregister".to_owned()),
                args: ContractArgs::Json(serde_json::json!({ "force": false })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(1),
            },
        })
        .await?;

    let unregister_result = stack
        .controller
        .request::<tx::Get>(&tx::Get {
            tx_hash: tx_hash(&unregister_transaction),
            sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
            wait_until: None,
            encoding: tx::ValueEncoding::Base64,
        })
        .await?;

    assert_eq!(unregister_result.status, tx::Status::Succeeded);
    assert!(matches!(
        unregister_result.return_value,
        Some(tx::ReturnValue::Base64(_))
    ));

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn storage_ensure_deposit_endpoint_supports_noop_and_operation() -> Result<()> {
    let stack = TestStack::start().await?;

    let first = stack
        .controller
        .request::<storage::EnsureDeposit>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: storage::EnsureDeposit {
                contract_id: stack.harness.ft_contract_id.clone(),
                account_id: stack.harness.beneficiary_account_id.clone(),
                mode: storage::EnsureDepositMode::Registered,
            },
        })
        .await?;

    assert_eq!(first.operation.steps.len(), 1);

    let second = stack
        .controller
        .request::<storage::EnsureDeposit>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: storage::EnsureDeposit {
                contract_id: stack.harness.ft_contract_id.clone(),
                account_id: stack.harness.beneficiary_account_id.clone(),
                mode: storage::EnsureDepositMode::Registered,
            },
        })
        .await?;

    assert_eq!(second.operation.steps.len(), 0);

    stack.shutdown().await;
    Ok(())
}
