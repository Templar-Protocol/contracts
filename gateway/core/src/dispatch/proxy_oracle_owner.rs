use futures::future::BoxFuture;
use templar_gateway_types::proxy_oracle_owner;

use crate::{
    client::{proxy_oracle::OwnerProposeArgs, ContractWriteOptions},
    dispatch::single_transaction_plan,
    operation::OperationPlan,
    GatewayContext, GatewayResult,
};
use crate::{DispatchRead, PlanWrite};

impl DispatchRead<GatewayContext> for proxy_oracle_owner::GetOwner {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.proxy_oracle(request.params.oracle_id)
                .own_get_owner(())
                .await
                .map(|owner| proxy_oracle_owner::GetOwnerResult { owner })
        })
    }
}

impl DispatchRead<GatewayContext> for proxy_oracle_owner::GetProposedOwner {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.proxy_oracle(request.params.oracle_id)
                .own_get_proposed_owner(())
                .await
                .map(|proposed_owner| proxy_oracle_owner::GetProposedOwnerResult { proposed_owner })
        })
    }
}

impl PlanWrite<GatewayContext> for proxy_oracle_owner::ProposeOwner {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            Ok(single_transaction_plan(
                ctx.proxy_oracle(body.oracle_id).own_propose_owner(
                    ContractWriteOptions::new(request.signer_account_id)
                        .one_yocto()
                        .tgas(300),
                    OwnerProposeArgs {
                        account_id: body.account_id,
                    },
                )?,
            ))
        })
    }
}

impl PlanWrite<GatewayContext> for proxy_oracle_owner::AcceptOwner {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            Ok(single_transaction_plan(
                ctx.proxy_oracle(request.body.oracle_id).own_accept_owner(
                    ContractWriteOptions::new(request.signer_account_id)
                        .one_yocto()
                        .tgas(300),
                    (),
                )?,
            ))
        })
    }
}

impl PlanWrite<GatewayContext> for proxy_oracle_owner::RenounceOwner {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            Ok(single_transaction_plan(
                ctx.proxy_oracle(request.body.oracle_id)
                    .own_renounce_owner(
                        ContractWriteOptions::new(request.signer_account_id)
                            .one_yocto()
                            .tgas(300),
                        (),
                    )?,
            ))
        })
    }
}
