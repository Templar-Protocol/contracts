use super::*;

use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_common::state::legacy::v0;
use templar_proxy_oracle_near_governance_common::{Operation, OperationKind};

#[tokio::test]
async fn proxy_oracle_governance_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let oracle_id = stack.harness.deploy_proxy_oracle().await?;
    let admin_id = stack.harness.proxy_oracle_signer_account_id.0.clone();
    let governance_id = stack
        .harness
        .deploy_governance_contract(oracle_id.clone(), admin_id)
        .await?;

    // The ownership handover consumed proposal id 0, so governance now starts at 1
    // with no active proposals.
    let next_id = stack
        .controller
        .request::<proxy_oracle_governance::NextProposalId>(&ReadRequest {
            params: proxy_oracle_governance::NextProposalIdParams {
                governance_id: governance_id.clone(),
            },
        })
        .await?;
    assert_eq!(next_id, 1);

    let count = stack
        .controller
        .request::<proxy_oracle_governance::ProposalCount>(&ReadRequest {
            params: proxy_oracle_governance::ProposalCountParams {
                governance_id: governance_id.clone(),
            },
        })
        .await?;
    assert_eq!(count, 0);

    let ttl = stack
        .controller
        .request::<proxy_oracle_governance::GetOperationTtl>(&ReadRequest {
            params: proxy_oracle_governance::GetOperationTtlParams {
                governance_id: governance_id.clone(),
                kind: OperationKind::SetProxy,
            },
        })
        .await?;
    assert_eq!(ttl.ttl_ns, Nanoseconds::zero());

    // Create a SetProxy proposal (id 1).
    let price_id = PriceIdentifier([0xaa; 32]);
    let proxy = Proxy::median_low(
        [OracleRequest::pyth(
            "pyth.near".parse().expect("valid oracle id"),
            PriceIdentifier([0xbb; 32]),
        )
        .into()],
        FreshnessFilter::empty(),
    );
    let _ = stack
        .controller
        .request::<proxy_oracle_governance::CreateProposal>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::CreateProposalBody {
                governance_id: governance_id.clone(),
                id: 1,
                operation: Operation::SetProxy {
                    id: price_id,
                    proxy: Some(proxy.clone()),
                },
                requested_ttl: Nanoseconds::zero(),
            },
        })
        .await?;

    let proposal = stack
        .controller
        .request::<proxy_oracle_governance::GetProposal>(&ReadRequest {
            params: proxy_oracle_governance::GetProposalParams {
                governance_id: governance_id.clone(),
                id: 1,
            },
        })
        .await?;
    assert!(proposal.proposal.is_some());
    let ids = stack
        .controller
        .request::<proxy_oracle_governance::ListProposals>(&ReadRequest {
            params: proxy_oracle_governance::ListProposalsParams {
                governance_id: governance_id.clone(),
                offset: None,
                count: None,
            },
        })
        .await?;
    assert_eq!(ids.ids, vec![1]);

    // Execute it: governance drives `admin_set_proxy` on the oracle it owns.
    let _ = stack
        .controller
        .request::<proxy_oracle_governance::ExecuteProposal>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::ExecuteProposalBody {
                governance_id: governance_id.clone(),
                id: 1,
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

    // Create then cancel another proposal (id 2).
    let _ = stack
        .controller
        .request::<proxy_oracle_governance::CreateProposal>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::CreateProposalBody {
                governance_id: governance_id.clone(),
                id: 2,
                operation: Operation::SetActionTtl {
                    kind: OperationKind::SetProxy,
                    new_ttl: Nanoseconds::from_secs(1),
                },
                requested_ttl: Nanoseconds::zero(),
            },
        })
        .await?;
    let _ = stack
        .controller
        .request::<proxy_oracle_governance::CancelProposal>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::CancelProposalBody {
                governance_id: governance_id.clone(),
                id: 2,
            },
        })
        .await?;
    let cancelled = stack
        .controller
        .request::<proxy_oracle_governance::GetProposal>(&ReadRequest {
            params: proxy_oracle_governance::GetProposalParams {
                governance_id,
                id: 2,
            },
        })
        .await?;
    assert!(cancelled.proposal.is_none());

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn proxy_oracle_owner_endpoints_work_against_sandbox() -> Result<()> {
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

    let _ = stack
        .controller
        .request::<proxy_oracle_owner::ProposeOwner>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
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
            body: proxy_oracle_owner::AcceptOwnerBody {
                oracle_id: oracle_id.clone(),
            },
        })
        .await?;

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
        Some(stack.harness.cleanup_signer_account_id.0.clone())
    );

    let _ = stack
        .controller
        .request::<proxy_oracle_owner::RenounceOwner>(&WriteRequest {
            signer_account_id: stack.harness.cleanup_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_owner::RenounceOwnerBody {
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
    assert_eq!(owner.owner, None);

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn proxy_oracle_get_proxy_normalizes_legacy_v0() -> Result<()> {
    let stack = TestStack::start().await?;
    let oracle_id = stack.harness.deploy_legacy_v0_proxy_oracle().await?;

    let price_id = PriceIdentifier([0xcc; 32]);
    let v0_proxy = v0::Proxy {
        aggregator: v0::Aggregator::median_low(v0::Filter::default()),
        entries: vec![v0::Entry::new(
            OracleRequest::pyth(
                "pyth.near".parse().expect("valid oracle id"),
                PriceIdentifier([0xdd; 32]),
            ),
            1,
        )],
    };
    // The gateway must surface the unified `Proxy<Source>`; the canonical
    // legacy->kernel conversion is the source of truth for the expected value.
    let expected: Proxy<Source> = Proxy::from(v0_proxy.clone());

    stack
        .harness
        .seed_legacy_v0_proxy(oracle_id.clone(), price_id, v0_proxy)
        .await?;

    let got = stack
        .controller
        .request::<proxy_oracle::GetProxy>(&ReadRequest {
            params: proxy_oracle::GetProxyParams {
                oracle_id: oracle_id.clone(),
                id: price_id,
            },
        })
        .await?;
    assert_eq!(got.proxy, Some(expected));

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

    let list = stack
        .controller
        .request::<proxy_oracle::ListProxies>(&ReadRequest {
            params: proxy_oracle::ListProxiesParams {
                oracle_id,
                offset: None,
                count: None,
            },
        })
        .await?;
    assert_eq!(list.proxies, vec![price_id]);

    stack.shutdown().await;
    Ok(())
}
