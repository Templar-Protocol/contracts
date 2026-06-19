use super::*;

#[tokio::test]
async fn token_endpoints_work_for_ft_and_mt_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let mt_contract_id = stack.harness.deploy_mt("token-mt.near".parse()?).await?;
    let receiver_id = stack
        .harness
        .deploy_receiver("token-receiver.near".parse()?)
        .await?;

    let _ = register_gateway_signer_for_ft(&stack).await?;
    let _ = register_ft_account(&stack, receiver_id.clone()).await?;
    let _ = register_ft_account(&stack, stack.harness.beneficiary_account_id.clone()).await?;

    let _ = stack
        .controller
        .request::<tx::FunctionCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: tx::FunctionCall {
                receiver_id: stack.harness.ft_contract_id.clone(),
                method_name: ContractMethodName("mint".to_owned()),
                args: ContractArgs::Json(serde_json::json!({ "amount": "5" })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::ZERO,
            },
        })
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
                    "amount": "6",
                })),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::ZERO,
            },
        })
        .await?;

    let ft_balance = stack
        .controller
        .request::<token::GetBalanceOf>(&ReadRequest {
            params: token::GetBalanceOf {
                token: token::TokenReference::Ft {
                    contract_id: stack.harness.ft_contract_id.clone(),
                },
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
            },
        })
        .await?;
    assert_eq!(ft_balance.balance, templar_gateway_types::U128(5));

    let mt_balance = stack
        .controller
        .request::<token::GetBalanceOf>(&ReadRequest {
            params: token::GetBalanceOf {
                token: token::TokenReference::Mt {
                    contract_id: mt_contract_id.clone(),
                    token_id: "mt_borrow".to_owned(),
                },
                account_id: stack.harness.gateway_signer_account_id.0.clone(),
            },
        })
        .await?;
    assert_eq!(mt_balance.balance, templar_gateway_types::U128(6));

    let transfer = stack
        .controller
        .request::<token::Transfer>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: token::Transfer {
                token: token::TokenReference::Ft {
                    contract_id: stack.harness.ft_contract_id.clone(),
                },
                receiver_id: stack.harness.beneficiary_account_id.clone(),
                amount: templar_gateway_types::U128(5),
                memo: Some("token-transfer".to_owned()),
            },
        })
        .await?;
    assert_eq!(
        transfer.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    let transfer_call = stack
        .controller
        .request::<token::TransferCall>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: token::TransferCall {
                token: token::TokenReference::Mt {
                    contract_id: mt_contract_id.clone(),
                    token_id: "mt_borrow".to_owned(),
                },
                receiver_id: receiver_id.clone(),
                amount: templar_gateway_types::U128(6),
                msg: "ok".to_owned(),
                memo: Some("token-call".to_owned()),
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
