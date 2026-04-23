use futures::future::BoxFuture;
use templar_gateway_types::proxy_oracle_governance;

use crate::{
    actor::{DispatchRead, PlanWrite},
    client::{
        proxy_oracle::{GovActionArgs, GovCreateArgs, GovGetArgs, GovListArgs},
        ContractWriteOptions,
    },
    dispatch::single_transaction_plan,
    operation::OperationPlan,
    GatewayContext, GatewayResult,
};

impl DispatchRead for proxy_oracle_governance::GetNextId {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.proxy_oracle(request.params.oracle_id)
                .gov_next_id(())
                .await
        })
    }
}

impl DispatchRead for proxy_oracle_governance::GetTtl {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let ttl_ns = ctx
                .proxy_oracle(request.params.oracle_id)
                .gov_ttl_ns(())
                .await?;
            Ok(Self::Output { ttl_ns })
        })
    }
}

impl DispatchRead for proxy_oracle_governance::GetCount {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.proxy_oracle(request.params.oracle_id)
                .gov_count(())
                .await
        })
    }
}

impl DispatchRead for proxy_oracle_governance::List {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.proxy_oracle(request.params.oracle_id)
                .gov_list(GovListArgs {
                    offset: request.params.offset,
                    count: request.params.count,
                })
                .await
                .map(|ids| proxy_oracle_governance::ListResult { ids })
        })
    }
}

impl DispatchRead for proxy_oracle_governance::Get {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            ctx.proxy_oracle(params.oracle_id)
                .gov_get(GovGetArgs { id: params.id })
                .await
                .map(|proposal| proxy_oracle_governance::GetResult { proposal })
        })
    }
}

impl PlanWrite for proxy_oracle_governance::Create {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            Ok(single_transaction_plan(
                ctx.proxy_oracle(body.oracle_id).gov_create(
                    ContractWriteOptions::new(request.signer_account_id)
                        .one_yocto()
                        .tgas(300),
                    GovCreateArgs {
                        id: body.id,
                        operation: body.operation,
                    },
                )?,
            ))
        })
    }
}

impl PlanWrite for proxy_oracle_governance::Cancel {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            Ok(single_transaction_plan(
                ctx.proxy_oracle(body.oracle_id).gov_cancel(
                    ContractWriteOptions::new(request.signer_account_id)
                        .one_yocto()
                        .tgas(300),
                    GovActionArgs { id: body.id },
                )?,
            ))
        })
    }
}

impl PlanWrite for proxy_oracle_governance::Execute {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            Ok(single_transaction_plan(
                ctx.proxy_oracle(body.oracle_id).gov_execute(
                    ContractWriteOptions::new(request.signer_account_id)
                        .one_yocto()
                        .tgas(300),
                    GovActionArgs { id: body.id },
                )?,
            ))
        })
    }
}
