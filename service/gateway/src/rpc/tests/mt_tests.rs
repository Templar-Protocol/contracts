use super::*;

#[tokio::test]
async fn mt_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let mt_contract_id = stack.harness.deploy_mt("mock-mt.near".parse()?).await?;
    let receiver_id = stack
        .harness
        .deploy_receiver("mt-receiver.near".parse()?)
        .await?;

    let _ = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: tx::FunctionCall {
                receiver_id: mt_contract_id.clone(),
                method_name: ContractMethodName("mint".to_owned()),
                args: ContractArgs::Json(serde_json::json!({
                    "token_id": "mt_borrow",
                    "amount": "11",
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::ZERO,
            },
        })
        .await?;

    let balance = stack
        .controller
        .request::<mt::GetBalanceOf>(&ReadRequest {
            params: mt::GetBalanceOf {
                contract_id: mt_contract_id.clone(),
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
                token_id: "mt_borrow".to_owned(),
            },
        })
        .await?;
    assert_eq!(balance.balance, templar_gateway_types::U128(11));

    let balances = stack
        .controller
        .request::<mt::GetBatchBalanceOf>(&ReadRequest {
            params: mt::GetBatchBalanceOf {
                contract_id: mt_contract_id.clone(),
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
                token_ids: vec!["mt_borrow".to_owned(), "mt_collateral".to_owned()],
            },
        })
        .await?;
    assert_eq!(
        balances.balances[0].balance,
        templar_gateway_types::U128(11)
    );
    assert_eq!(balances.balances[1].balance, templar_gateway_types::U128(0));

    let supply = stack
        .controller
        .request::<mt::GetSupply>(&ReadRequest {
            params: mt::GetSupply {
                contract_id: mt_contract_id.clone(),
                token_id: "mt_borrow".to_owned(),
            },
        })
        .await?;
    assert_eq!(supply.supply, Some(templar_gateway_types::U128(11)));

    let supplies = stack
        .controller
        .request::<mt::GetBatchSupply>(&ReadRequest {
            params: mt::GetBatchSupply {
                contract_id: mt_contract_id.clone(),
                token_ids: vec!["mt_borrow".to_owned(), "missing".to_owned()],
            },
        })
        .await?;
    assert_eq!(
        supplies.supplies[0].supply,
        Some(templar_gateway_types::U128(11))
    );
    assert_eq!(supplies.supplies[1].supply, None);

    let transfer = stack
        .controller
        .request::<mt::Transfer>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: mt::Transfer {
                contract_id: mt_contract_id.clone(),
                receiver_id: stack.harness.beneficiary_account_id.clone(),
                token_id: "mt_borrow".to_owned(),
                amount: templar_gateway_types::U128(4),
                approval: None,
                memo: Some("transfer".to_owned()),
            },
        })
        .await?;
    assert_eq!(
        transfer.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    let transfer_call = stack
        .controller
        .request::<mt::TransferCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: mt::TransferCall {
                contract_id: mt_contract_id.clone(),
                receiver_id: receiver_id.clone(),
                token_id: "mt_borrow".to_owned(),
                amount: templar_gateway_types::U128(7),
                approval: None,
                memo: Some("call".to_owned()),
                msg: "ok".to_owned(),
            },
        })
        .await?;
    assert_eq!(
        transfer_call.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    let _ = stack
        .controller
        .request::<tx::Get>(&ReadRequest {
            params: tx::Get {
                tx_hash: tx_hash(&transfer_call),
                sender_account_id: stack.harness.gateway_signer_account_id.0.clone(),
                wait_until: Some(templar_gateway_types::common::TxExecutionStatus::Final),
                encoding: tx::ValueEncoding::Json,
            },
        })
        .await?;

    stack.shutdown().await;
    Ok(())
}
