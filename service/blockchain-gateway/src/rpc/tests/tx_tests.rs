use super::*;

#[tokio::test]
async fn tx_function_call_and_view_function_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;

    let _ = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: tx::FunctionCallBody {
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
        .request::<contract::ViewFunction>(&ReadRequest {
            params: contract::ViewFunctionParams {
                contract_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("redemption_rate".to_owned()),
                args: ContractArgs::Raw(Base64Bytes(Vec::new())),
            },
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
async fn tx_transfer_unregister_and_account_delete_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;

    let _ = register_gateway_signer_for_ft(&stack).await?;

    let _ = stack
        .controller
        .request::<storage::Deposit>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: storage::DepositBody {
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
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: tx::FunctionCallBody {
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
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: ft::TransferBody {
                contract_id: stack.harness.ft_contract_id.clone(),
                receiver_id: stack.harness.beneficiary_account_id.clone(),
                amount: blockchain_gateway_core::U128(3),
            },
        })
        .await?;

    let balance = stack
        .controller
        .request::<ft::GetBalanceOf>(&ReadRequest {
            params: ft::GetBalanceOfParams {
                contract_id: stack.harness.ft_contract_id.clone(),
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
            },
        })
        .await?;

    assert_eq!(balance.balance, blockchain_gateway_core::U128(0));

    let _ = stack
        .controller
        .request::<storage::Unregister>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: storage::UnregisterBody {
                contract_id: stack.harness.ft_contract_id.clone(),
                force: false,
            },
        })
        .await?;

    let storage_balance = stack
        .controller
        .request::<storage::GetBalanceOf>(&ReadRequest {
            params: storage::GetBalanceOfParams {
                contract_id: stack.harness.ft_contract_id.clone(),
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
            },
        })
        .await?;

    assert!(storage_balance.balance.is_none());

    let _ = stack
        .controller
        .request::<account::Delete>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: account::DeleteBody {
                beneficiary_id: stack.harness.beneficiary_account_id.clone(),
            },
        })
        .await?;

    let deleted = stack
        .controller
        .request::<tx::Get>(&ReadRequest {
            params: tx::GetParams {
                tx_hash: CryptoHash(
                    "11111111111111111111111111111111"
                        .parse()
                        .expect("valid dummy hash"),
                ),
                sender_account_id: stack.harness.cleanup_signer_account_id.0.clone(),
                wait_until: Some(blockchain_gateway_core::common::TxExecutionStatus::None),
                encoding: tx::ValueEncoding::Json,
            },
        })
        .await;

    assert!(deleted.is_err());

    stack.shutdown().await;
    Ok(())
}
