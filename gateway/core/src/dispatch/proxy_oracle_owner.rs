use futures::future::BoxFuture;
use templar_gateway_types::proxy_oracle_owner;

use crate::{
    client::{proxy_oracle::OwnerProposeArgs, ContractWriteOptions},
    operation::OperationPlan,
    GatewayResult, HasNearClient,
};
use crate::{DispatchRead, PlanWrite};

impl<C: HasNearClient> DispatchRead<C> for proxy_oracle_owner::GetOwner {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .proxy_oracle(request.params.oracle_id)
                .own_get_owner(())
                .await
                .map(|owner| proxy_oracle_owner::GetOwnerResult { owner })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for proxy_oracle_owner::GetProposedOwner {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .proxy_oracle(request.params.oracle_id)
                .own_get_proposed_owner(())
                .await
                .map(|proposed_owner| proxy_oracle_owner::GetProposedOwnerResult { proposed_owner })
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for proxy_oracle_owner::ProposeOwner {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            ctx.near_client()
                .proxy_oracle(body.oracle_id)
                .own_propose_owner(
                    ContractWriteOptions::new(request.signer_account_id)
                        .one_yocto()
                        .tgas(300),
                    OwnerProposeArgs {
                        account_id: body.account_id,
                    },
                )
                .map(OperationPlan::from)
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for proxy_oracle_owner::AcceptOwner {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            ctx.near_client()
                .proxy_oracle(request.body.oracle_id)
                .own_accept_owner(
                    ContractWriteOptions::new(request.signer_account_id)
                        .one_yocto()
                        .tgas(300),
                    (),
                )
                .map(OperationPlan::from)
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for proxy_oracle_owner::RenounceOwner {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            ctx.near_client()
                .proxy_oracle(request.body.oracle_id)
                .own_renounce_owner(
                    ContractWriteOptions::new(request.signer_account_id)
                        .one_yocto()
                        .tgas(300),
                    (),
                )
                .map(OperationPlan::from)
        })
    }
}
