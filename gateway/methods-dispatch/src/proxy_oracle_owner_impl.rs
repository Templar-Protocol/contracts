use async_trait::async_trait;
use templar_gateway_core::{
    client::{proxy_oracle::OwnerProposeArgs, ContractWriteOptions},
    DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::proxy_oracle_owner;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_owner::GetOwner, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_owner::GetOwner,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_owner::GetOwnerResult> {
        ctx.near_client()
            .proxy_oracle(request.oracle_id)
            .own_get_owner(())
            .await
            .map(|owner| proxy_oracle_owner::GetOwnerResult { owner })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_owner::GetProposedOwner, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_owner::GetProposedOwner,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_owner::GetProposedOwnerResult> {
        ctx.near_client()
            .proxy_oracle(request.oracle_id)
            .own_get_proposed_owner(())
            .await
            .map(|proposed_owner| proxy_oracle_owner::GetProposedOwnerResult { proposed_owner })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_owner::ProposeOwner, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<proxy_oracle_owner::ProposeOwner>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
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
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_owner::AcceptOwner, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<proxy_oracle_owner::AcceptOwner>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .proxy_oracle(request.body.oracle_id)
            .own_accept_owner(
                ContractWriteOptions::new(request.signer_account_id)
                    .one_yocto()
                    .tgas(300),
                (),
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_owner::RenounceOwner, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<proxy_oracle_owner::RenounceOwner>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .proxy_oracle(request.body.oracle_id)
            .own_renounce_owner(
                ContractWriteOptions::new(request.signer_account_id)
                    .one_yocto()
                    .tgas(300),
                (),
            )
            .map(OperationPlan::from)
    }
}
