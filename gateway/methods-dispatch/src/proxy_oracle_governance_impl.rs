use async_trait::async_trait;
use templar_gateway_core::{
    client::{
        proxy_governance::{GovActionArgs, GovCreateArgs, GovGetArgs, GovListArgs, GovTtlArgs},
        ContractWriteOptions,
    },
    DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::proxy_oracle_governance;
use templar_gateway_types::MethodSpec;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::NextProposalId, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle_governance::NextProposalId as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::NextProposalIdResult> {
        ctx.near_client()
            .proxy_governance(request.params.governance_id)
            .next_proposal_id(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::ProposalCount, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle_governance::ProposalCount as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::ProposalCountResult> {
        ctx.near_client()
            .proxy_governance(request.params.governance_id)
            .proposal_count(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetOperationTtl, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle_governance::GetOperationTtl as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetOperationTtlResult> {
        let params = request.params;
        let ttl_ns = ctx
            .near_client()
            .proxy_governance(params.governance_id)
            .get_operation_ttl(GovTtlArgs { kind: params.kind })
            .await?;
        Ok(proxy_oracle_governance::GetOperationTtlResult { ttl_ns })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::ListProposals, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle_governance::ListProposals as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::ListProposalsResult> {
        ctx.near_client()
            .proxy_governance(request.params.governance_id)
            .list_proposals(GovListArgs {
                offset: request.params.offset,
                count: request.params.count,
            })
            .await
            .map(|ids| proxy_oracle_governance::ListProposalsResult { ids })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetProposal, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle_governance::GetProposal as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetProposalResult> {
        let params = request.params;
        ctx.near_client()
            .proxy_governance(params.governance_id)
            .get_proposal(GovGetArgs { id: params.id })
            .await
            .map(|proposal| proxy_oracle_governance::GetProposalResult { proposal })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_governance::CreateProposal, C> for Dispatch {
    async fn plan(
        request: <proxy_oracle_governance::CreateProposal as MethodSpec>::Input,
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
        request: <proxy_oracle_governance::CancelProposal as MethodSpec>::Input,
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
        request: <proxy_oracle_governance::ExecuteProposal as MethodSpec>::Input,
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
