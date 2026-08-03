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
        ctx.near_client()
            .proxy_governance(body.governance_id)
            .create_proposal(
                ContractWriteOptions::new(request.signer_account_id)
                    .one_yocto()
                    .tgas(300),
                GovCreateArgs {
                    id: body.id,
                    operation: body.operation,
                    requested_ttl: body.requested_ttl,
                },
            )
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
