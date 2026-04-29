use async_trait::async_trait;
use templar_gateway_types::{proxy_oracle_governance, MethodSpec};

use super::Dispatch;
use crate::{
    client::{
        proxy_oracle::{GovActionArgs, GovCreateArgs, GovGetArgs, GovListArgs},
        ContractWriteOptions,
    },
    operation::OperationPlan,
    DispatchRead, GatewayResult, HasNearClient, PlanWrite,
};

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetNextId, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle_governance::GetNextId as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetNextIdResult> {
        ctx.near_client()
            .proxy_oracle(request.params.oracle_id)
            .gov_next_id(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetTtl, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle_governance::GetTtl as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetTtlResult> {
        let ttl_ns = ctx
            .near_client()
            .proxy_oracle(request.params.oracle_id)
            .gov_ttl_ns(())
            .await?;
        Ok(proxy_oracle_governance::GetTtlResult { ttl_ns })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetCount, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle_governance::GetCount as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetCountResult> {
        ctx.near_client()
            .proxy_oracle(request.params.oracle_id)
            .gov_count(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::List, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle_governance::List as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::ListResult> {
        ctx.near_client()
            .proxy_oracle(request.params.oracle_id)
            .gov_list(GovListArgs {
                offset: request.params.offset,
                count: request.params.count,
            })
            .await
            .map(|ids| proxy_oracle_governance::ListResult { ids })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::Get, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle_governance::Get as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetResult> {
        let params = request.params;
        ctx.near_client()
            .proxy_oracle(params.oracle_id)
            .gov_get(GovGetArgs { id: params.id })
            .await
            .map(|proposal| proxy_oracle_governance::GetResult { proposal })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_governance::Create, C> for Dispatch {
    async fn plan(
        request: <proxy_oracle_governance::Create as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .proxy_oracle(body.oracle_id)
            .gov_create(
                ContractWriteOptions::new(request.signer_account_id)
                    .one_yocto()
                    .tgas(300),
                GovCreateArgs {
                    id: body.id,
                    operation: body.operation,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_governance::Cancel, C> for Dispatch {
    async fn plan(
        request: <proxy_oracle_governance::Cancel as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .proxy_oracle(body.oracle_id)
            .gov_cancel(
                ContractWriteOptions::new(request.signer_account_id)
                    .one_yocto()
                    .tgas(300),
                GovActionArgs { id: body.id },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_governance::Execute, C> for Dispatch {
    async fn plan(
        request: <proxy_oracle_governance::Execute as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .proxy_oracle(body.oracle_id)
            .gov_execute(
                ContractWriteOptions::new(request.signer_account_id)
                    .one_yocto()
                    .tgas(300),
                GovActionArgs { id: body.id },
            )
            .map(OperationPlan::from)
    }
}
