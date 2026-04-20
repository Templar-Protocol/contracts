use super::*;

#[tokio::test]
async fn proxy_oracle_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let oracle_id = stack.harness.deploy_proxy_oracle().await?;

    let owner = stack
        .controller
        .request::<proxy_oracle_owner::GetOwner>(&ReadRequest {
            params: proxy_oracle_owner::GetOwnerParams {
                oracle_id: oracle_id.clone(),
            },
        })
        .await?;
    assert_eq!(
        owner.owner,
        Some(stack.harness.proxy_oracle_signer_account_id.0.clone())
    );

    let next_id = stack
        .controller
        .request::<proxy_oracle_governance::GetNextId>(&ReadRequest {
            params: proxy_oracle_governance::GetNextIdParams {
                oracle_id: oracle_id.clone(),
            },
        })
        .await?;
    assert_eq!(next_id, 0);
    let ttl = stack
        .controller
        .request::<proxy_oracle_governance::GetTtl>(&ReadRequest {
            params: proxy_oracle_governance::GetTtlParams {
                oracle_id: oracle_id.clone(),
            },
        })
        .await?;
    assert_eq!(ttl.ttl_ns, templar_common::time::Nanoseconds::zero());
    let count = stack
        .controller
        .request::<proxy_oracle_governance::GetCount>(&ReadRequest {
            params: proxy_oracle_governance::GetCountParams {
                oracle_id: oracle_id.clone(),
            },
        })
        .await?;
    assert_eq!(count, 0);

    let list = stack
        .controller
        .request::<proxy_oracle::ListProxies>(&ReadRequest {
            params: proxy_oracle::ListProxiesParams {
                oracle_id: oracle_id.clone(),
                offset: None,
                count: None,
            },
        })
        .await?;
    assert!(list.proxies.is_empty());

    let price_id = templar_common::oracle::pyth::PriceIdentifier([0xaa; 32]);
    let proxy = templar_common::oracle::proxy::Proxy::median_low([
        templar_common::oracle::OracleRequest::pyth(
            "pyth.near".parse().expect("valid oracle id"),
            templar_common::oracle::pyth::PriceIdentifier([0xbb; 32]),
        )
        .into(),
    ]);

    let _ = stack
        .controller
        .request::<proxy_oracle_governance::Create>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: proxy_oracle_governance::CreateBody {
                oracle_id: oracle_id.clone(),
                id: 0,
                operation: templar_common::oracle::proxy::governance::Operation::SetProxy {
                    id: price_id,
                    proxy: Some(proxy.clone()),
                },
            },
        })
        .await?;

    let proposal = stack
        .controller
        .request::<proxy_oracle_governance::Get>(&ReadRequest {
            params: proxy_oracle_governance::GetParams {
                oracle_id: oracle_id.clone(),
                id: 0,
            },
        })
        .await?;
    assert!(proposal.proposal.is_some());
    let ids = stack
        .controller
        .request::<proxy_oracle_governance::List>(&ReadRequest {
            params: proxy_oracle_governance::ListParams {
                oracle_id: oracle_id.clone(),
                offset: None,
                count: None,
            },
        })
        .await?;
    assert_eq!(ids.ids, vec![0]);

    let _ = stack
        .controller
        .request::<proxy_oracle_governance::Execute>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: proxy_oracle_governance::ExecuteBody {
                oracle_id: oracle_id.clone(),
                id: 0,
            },
        })
        .await?;

    let got_proxy = stack
        .controller
        .request::<proxy_oracle::GetProxy>(&ReadRequest {
            params: proxy_oracle::GetProxyParams {
                oracle_id: oracle_id.clone(),
                id: price_id,
            },
        })
        .await?;
    assert_eq!(got_proxy.proxy, Some(proxy));

    let exists = stack
        .controller
        .request::<proxy_oracle::PriceFeedExists>(&ReadRequest {
            params: proxy_oracle::PriceFeedExistsParams {
                oracle_id: oracle_id.clone(),
                price_identifier: price_id,
            },
        })
        .await?;
    assert!(exists.exists);

    let _ = stack
        .controller
        .request::<proxy_oracle_owner::ProposeOwner>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: proxy_oracle_owner::ProposeOwnerBody {
                oracle_id: oracle_id.clone(),
                account_id: Some(stack.harness.cleanup_signer_account_id.0.clone()),
            },
        })
        .await?;

    let proposed = stack
        .controller
        .request::<proxy_oracle_owner::GetProposedOwner>(&ReadRequest {
            params: proxy_oracle_owner::GetProposedOwnerParams {
                oracle_id: oracle_id.clone(),
            },
        })
        .await?;
    assert_eq!(
        proposed.proposed_owner,
        Some(stack.harness.cleanup_signer_account_id.0.clone())
    );

    let _ = stack
        .controller
        .request::<proxy_oracle_owner::AcceptOwner>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: proxy_oracle_owner::AcceptOwnerBody {
                oracle_id: oracle_id.clone(),
            },
        })
        .await?;

    let owner = stack
        .controller
        .request::<proxy_oracle_owner::GetOwner>(&ReadRequest {
            params: proxy_oracle_owner::GetOwnerParams { oracle_id },
        })
        .await?;
    assert_eq!(
        owner.owner,
        Some(stack.harness.cleanup_signer_account_id.0.clone())
    );

    let _ = stack
        .controller
        .request::<proxy_oracle_governance::Create>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: proxy_oracle_governance::CreateBody {
                oracle_id: stack.harness.proxy_oracle_signer_account_id.0.clone(),
                id: 1,
                operation: templar_common::oracle::proxy::governance::Operation::SetActionTtl {
                    new_ttl: templar_common::time::Nanoseconds::from_secs(1),
                },
            },
        })
        .await?;
    let _ = stack
        .controller
        .request::<proxy_oracle_governance::Cancel>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: proxy_oracle_governance::CancelBody {
                oracle_id: stack.harness.proxy_oracle_signer_account_id.0.clone(),
                id: 1,
            },
        })
        .await?;
    let cancelled = stack
        .controller
        .request::<proxy_oracle_governance::Get>(&ReadRequest {
            params: proxy_oracle_governance::GetParams {
                oracle_id: stack.harness.proxy_oracle_signer_account_id.0.clone(),
                id: 1,
            },
        })
        .await?;
    assert!(cancelled.proposal.is_none());

    let _ = stack
        .controller
        .request::<proxy_oracle_owner::RenounceOwner>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            wait_until: blockchain_gateway_core::common::TxExecutionStatus::Final,
            body: proxy_oracle_owner::RenounceOwnerBody {
                oracle_id: stack.harness.proxy_oracle_signer_account_id.0.clone(),
            },
        })
        .await?;
    let owner = stack
        .controller
        .request::<proxy_oracle_owner::GetOwner>(&ReadRequest {
            params: proxy_oracle_owner::GetOwnerParams {
                oracle_id: stack.harness.proxy_oracle_signer_account_id.0.clone(),
            },
        })
        .await?;
    assert_eq!(owner.owner, None);

    stack.shutdown().await;
    Ok(())
}
