use super::*;

use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_common::state::legacy::v0;
use templar_proxy_oracle_near_governance_common::{
    LegacyOperation, LegacyOperationKind, Operation,
};

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
        .request::<proxy_oracle_governance::NextProposalId>(
            &proxy_oracle_governance::NextProposalId {
                governance_id: governance_id.clone(),
            },
        )
        .await?;
    assert_eq!(next_id, 1);

    let count = stack
        .controller
        .request::<proxy_oracle_governance::ProposalCount>(
            &proxy_oracle_governance::ProposalCount {
                governance_id: governance_id.clone(),
            },
        )
        .await?;
    assert_eq!(count, 0);

    let policy = stack
        .controller
        .request::<proxy_oracle_governance::GetGovernancePolicy>(
            &proxy_oracle_governance::GetGovernancePolicy {
                governance_id: governance_id.clone(),
            },
        )
        .await?;
    assert_eq!(
        policy.policy.resolve("admin_set_proxy").ttl,
        Nanoseconds::zero()
    );

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
            body: proxy_oracle_governance::CreateProposal {
                governance_id: governance_id.clone(),
                id: 1,
                operation: Operation::try_from(LegacyOperation::SetProxy {
                    id: price_id,
                    proxy: Some(proxy.clone()),
                })
                .expect("build set-proxy operation"),
                requested_ttl: Nanoseconds::zero(),
            },
        })
        .await?;

    let proposal = stack
        .controller
        .request::<proxy_oracle_governance::GetProposal>(&proxy_oracle_governance::GetProposal {
            governance_id: governance_id.clone(),
            id: 1,
        })
        .await?;
    assert!(proposal.proposal.is_some());
    let ids = stack
        .controller
        .request::<proxy_oracle_governance::ListProposals>(
            &proxy_oracle_governance::ListProposals {
                governance_id: governance_id.clone(),
                offset: None,
                count: None,
            },
        )
        .await?;
    assert_eq!(ids.ids, vec![1]);

    // Execute it: governance drives `admin_set_proxy` on the oracle it owns.
    let _ = stack
        .controller
        .request::<proxy_oracle_governance::ExecuteProposal>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::ExecuteProposal {
                governance_id: governance_id.clone(),
                id: 1,
            },
        })
        .await?;

    let got_proxy = stack
        .controller
        .request::<proxy_oracle::GetProxy>(&proxy_oracle::GetProxy {
            oracle_id: oracle_id.clone(),
            id: price_id,
        })
        .await?;
    assert_eq!(got_proxy.proxy, Some(proxy));

    let exists = stack
        .controller
        .request::<proxy_oracle::PriceFeedExists>(&proxy_oracle::PriceFeedExists {
            oracle_id: oracle_id.clone(),
            price_identifier: price_id,
        })
        .await?;
    assert!(exists.exists);

    // Create then cancel another proposal (id 2).
    let _ = stack
        .controller
        .request::<proxy_oracle_governance::CreateProposal>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::CreateProposal {
                governance_id: governance_id.clone(),
                id: 2,
                operation: Operation::try_from(LegacyOperation::SetActionTtl {
                    kind: LegacyOperationKind::SetProxy,
                    new_ttl: Nanoseconds::from_secs(1),
                })
                .expect("build set-action-ttl operation"),
                requested_ttl: Nanoseconds::zero(),
            },
        })
        .await?;
    let _ = stack
        .controller
        .request::<proxy_oracle_governance::CancelProposal>(&WriteRequest {
            signer_account_id: stack.harness.proxy_oracle_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle_governance::CancelProposal {
                governance_id: governance_id.clone(),
                id: 2,
            },
        })
        .await?;
    let cancelled = stack
        .controller
        .request::<proxy_oracle_governance::GetProposal>(&proxy_oracle_governance::GetProposal {
            governance_id,
            id: 2,
        })
        .await?;
    assert!(cancelled.proposal.is_none());

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
        .request::<proxy_oracle::GetProxy>(&proxy_oracle::GetProxy {
            oracle_id: oracle_id.clone(),
            id: price_id,
        })
        .await?;
    assert_eq!(got.proxy, Some(expected));

    let exists = stack
        .controller
        .request::<proxy_oracle::PriceFeedExists>(&proxy_oracle::PriceFeedExists {
            oracle_id: oracle_id.clone(),
            price_identifier: price_id,
        })
        .await?;
    assert!(exists.exists);

    let list = stack
        .controller
        .request::<proxy_oracle::ListProxies>(&proxy_oracle::ListProxies {
            oracle_id,
            offset: None,
            count: None,
        })
        .await?;
    assert_eq!(list.proxies, vec![price_id]);

    stack.shutdown().await;
    Ok(())
}
