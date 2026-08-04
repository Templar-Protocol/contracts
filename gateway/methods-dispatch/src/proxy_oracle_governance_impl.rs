use async_trait::async_trait;
use templar_gateway_core::{
    client::{
        proxy_governance::{
            GovActionArgs, GovCreateArgs, GovGetArgs, GovGetRolesArgs, GovHasRoleArgs, GovListArgs,
            GovListRoleArgs,
        },
        ContractWriteOptions,
    },
    DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::proxy_oracle_governance;
use templar_gateway_types::{Governance, ProposalEncoding};

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::NextProposalId, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::NextProposalId,
        ctx: C,
    ) -> GatewayResult<u32> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .next_proposal_id(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::ProposalCount, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::ProposalCount,
        ctx: C,
    ) -> GatewayResult<u32> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .proposal_count(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetGovernancePolicy, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::GetGovernancePolicy,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetGovernancePolicyResult> {
        let policy = ctx
            .near_client()
            .proxy_governance(request.governance_id)
            .get_governance_policy(())
            .await?;
        Ok(proxy_oracle_governance::GetGovernancePolicyResult { policy })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::ListProposals, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::ListProposals,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::ListProposalsResult> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .list_proposals(GovListArgs {
                offset: request.offset,
                count: request.count,
            })
            .await
            .map(|ids| proxy_oracle_governance::ListProposalsResult { ids })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetProposal, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::GetProposal,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetProposalResult> {
        let params = request;
        ctx.near_client()
            .proxy_governance(params.governance_id)
            .get_proposal(GovGetArgs { id: params.id })
            .await
            .map(|proposal| proxy_oracle_governance::GetProposalResult { proposal })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetProxyOracleId, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::GetProxyOracleId,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetProxyOracleIdResult> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .get_proxy_oracle_id(())
            .await
            .map(
                |proxy_oracle_id| proxy_oracle_governance::GetProxyOracleIdResult {
                    proxy_oracle_id,
                },
            )
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::HasRole, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::HasRole,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::HasRoleResult> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .has_role(GovHasRoleArgs {
                account_id: request.account_id,
                role: request.role,
            })
            .await
            .map(|has_role| proxy_oracle_governance::HasRoleResult { has_role })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::ListRole, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::ListRole,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::ListRoleResult> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .list_role(GovListRoleArgs {
                role: request.role,
                offset: request.offset,
                count: request.count,
            })
            .await
            .map(|members| proxy_oracle_governance::ListRoleResult { members })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetRoles, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::GetRoles,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetRolesResult> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .get_roles(GovGetRolesArgs {
                account_id: request.account_id,
            })
            .await
            .map(|roles| proxy_oracle_governance::GetRolesResult { roles })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_governance::CreateProposal, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<
            proxy_oracle_governance::CreateProposal,
        >,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let governance_id = body.governance_id;
        let options = ContractWriteOptions::new(request.signer_account_id)
            .one_yocto()
            .tgas(300);
        let args = GovCreateArgs {
            id: body.id,
            operation: body.operation,
            requested_ttl: body.requested_ttl,
        };

        // Only the opt-in path pays for a version read, and uncached: the metadata cache holds for
        // an hour, which would refuse borsh for that long after a governance upgrade.
        let version = match body.encoding {
            ProposalEncoding::Json => None,
            ProposalEncoding::Borsh => Some(
                ctx.near_client()
                    .contract(governance_id.clone())
                    .version::<Governance>()
                    .await?,
            ),
        };

        let governance = ctx.near_client().proxy_governance(governance_id);
        match version {
            None => governance.create_proposal(options, args),
            Some(version) => governance.create_proposal_borsh(options, version, args),
        }
        .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_governance::CancelProposal, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<
            proxy_oracle_governance::CancelProposal,
        >,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .proxy_governance(body.governance_id)
            .cancel_proposal(
                ContractWriteOptions::new(request.signer_account_id)
                    .one_yocto()
                    .tgas(300),
                GovActionArgs { id: body.id },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_governance::ExecuteProposal, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<
            proxy_oracle_governance::ExecuteProposal,
        >,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .proxy_governance(body.governance_id)
            .execute_proposal(
                ContractWriteOptions::new(request.signer_account_id)
                    .one_yocto()
                    .tgas(300),
                GovActionArgs { id: body.id },
            )
            .map(OperationPlan::from)
    }
}

#[cfg(test)]
mod tests {
    use near_api::{types::transaction::actions::Action, NetworkConfig, RPCEndpoint};
    use templar_gateway_core::{HasNearClient, NearClient, PlanWrite};
    use templar_gateway_methods_spec::proxy_oracle_governance;
    use templar_gateway_types::{common::WriteRequest, ManagedAccountId, ProposalEncoding};
    use templar_proxy_oracle_near_governance_common::{target, Operation};

    use super::Dispatch;

    #[derive(Clone)]
    struct TestCtx(NearClient);

    impl HasNearClient for TestCtx {
        fn near_client(&self) -> &NearClient {
            &self.0
        }
    }

    /// Points at a closed port, so any path that touches the network fails.
    fn offline_ctx() -> TestCtx {
        // One retry, not the default five: the backoff would otherwise spend ~310ms per test
        // waiting out connection-refused on a port nothing is listening to.
        let mut network =
            NetworkConfig::from_rpc_url("test", "http://127.0.0.1:1".parse().unwrap());
        network.rpc_endpoints =
            vec![RPCEndpoint::new("http://127.0.0.1:1".parse().unwrap()).with_retries(1)];
        TestCtx(NearClient::new(network))
    }

    fn operation() -> Operation {
        Operation::TargetFunctionCall(
            target::admin_rearm(
                templar_common::oracle::pyth::PriceIdentifier([0xaa; 32]),
                0,
                templar_common::Nanoseconds::zero(),
                templar_proxy_oracle_kernel::proxy::circuit_breaker::AcceptedHistorySource::Empty,
                None,
            )
            .expect("build rearm call"),
        )
    }

    fn request(
        encoding: ProposalEncoding,
    ) -> WriteRequest<proxy_oracle_governance::CreateProposal> {
        WriteRequest {
            signer_account_id: ManagedAccountId("admin.near".parse().unwrap()),
            idempotency_key: None,
            body: proxy_oracle_governance::CreateProposal {
                governance_id: "gov.near".parse().unwrap(),
                id: 7,
                operation: operation(),
                requested_ttl: templar_common::Nanoseconds::zero(),
                encoding,
            },
        }
    }

    async fn plan(
        encoding: ProposalEncoding,
    ) -> templar_gateway_core::GatewayResult<templar_gateway_core::OperationPlan> {
        <Dispatch as PlanWrite<proxy_oracle_governance::CreateProposal, TestCtx>>::plan(
            request(encoding),
            offline_ctx(),
        )
        .await
    }

    /// Planning offline *is* the assertion: it can only succeed if no version was read.
    #[tokio::test]
    async fn json_encoding_plans_without_reading_a_version() {
        let plan = plan(ProposalEncoding::Json)
            .await
            .expect("json planning must not touch the network");

        let [Action::FunctionCall(action)] = &plan.steps[0].actions[..] else {
            panic!("expected one function call");
        };
        assert_eq!(action.method_name, "create_proposal");
        assert!(action.args.starts_with(b"{"), "expected json arguments");
    }

    /// The converse of the above: the same offline client fails, so opting in does read a version.
    #[tokio::test]
    async fn borsh_encoding_reads_the_version_first() {
        plan(ProposalEncoding::Borsh)
            .await
            .expect_err("borsh planning must consult the governance version");
    }
}
