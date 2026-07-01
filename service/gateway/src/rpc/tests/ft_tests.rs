use super::*;

#[tokio::test]
async fn ft_transfer_call_endpoint_works_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let receiver_id = stack
        .harness
        .deploy_receiver("ft-receiver.near".parse()?)
        .await?;

    let _ = register_gateway_signer_for_ft(&stack).await?;
    let _ = register_ft_account(&stack, receiver_id.clone()).await?;

    let _ = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: tx::FunctionCall {
                receiver_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("mint".to_owned()),
                args: ContractArgs::Json(serde_json::json!({ "amount": "7" })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::ZERO,
            },
        })
        .await?;

    let result = stack
        .controller
        .request::<ft::TransferCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: ft::TransferCall {
                contract_id: stack.harness.ft_contract_id.clone(),
                receiver_id: receiver_id.clone(),
                amount: templar_common::SU128::from(7),
                msg: "ok".to_owned(),
                memo: Some("gateway-test".to_owned()),
            },
        })
        .await?;

    assert_eq!(
        result.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    let _ = stack
        .controller
        .request::<tx::Get>(&tx::Get {
            tx_hash: tx_hash(&result),
            sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
            wait_until: Some(templar_gateway_types::common::TxExecutionStatus::Final),
            encoding: tx::ValueEncoding::Json,
        })
        .await?;

    stack.shutdown().await;
    Ok(())
}
