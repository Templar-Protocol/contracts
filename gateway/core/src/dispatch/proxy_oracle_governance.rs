use futures::future::BoxFuture;
use templar_gateway_types::proxy_oracle_governance;

use crate::{
    client::{
        proxy_oracle::{GovActionArgs, GovCreateArgs, GovGetArgs, GovListArgs},
        ContractWriteOptions,
    },
    operation::OperationPlan,
    GatewayResult, HasNearClient,
};
use crate::{DispatchRead, PlanWrite};

impl<C: HasNearClient> DispatchRead<C> for proxy_oracle_governance::GetNextId {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .proxy_oracle(request.params.oracle_id)
                .gov_next_id(())
                .await
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for proxy_oracle_governance::GetTtl {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let ttl_ns = ctx
                .near_client()
                .proxy_oracle(request.params.oracle_id)
                .gov_ttl_ns(())
                .await?;
            Ok(Self::Output { ttl_ns })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for proxy_oracle_governance::GetCount {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .proxy_oracle(request.params.oracle_id)
                .gov_count(())
                .await
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for proxy_oracle_governance::List {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .proxy_oracle(request.params.oracle_id)
                .gov_list(GovListArgs {
                    offset: request.params.offset,
                    count: request.params.count,
                })
                .await
                .map(|ids| proxy_oracle_governance::ListResult { ids })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for proxy_oracle_governance::Get {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            ctx.near_client()
                .proxy_oracle(params.oracle_id)
                .gov_get(GovGetArgs { id: params.id })
                .await
                .map(|proposal| proxy_oracle_governance::GetResult { proposal })
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for proxy_oracle_governance::Create {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
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
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for proxy_oracle_governance::Cancel {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
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
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for proxy_oracle_governance::Execute {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
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
        })
    }
}
