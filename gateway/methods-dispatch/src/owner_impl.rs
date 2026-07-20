use async_trait::async_trait;
use templar_gateway_core::{
    client::{owner::OwnerProposeArgs, ContractWriteOptions},
    DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::owner;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<owner::GetOwner, C> for Dispatch {
    async fn dispatch(request: owner::GetOwner, ctx: C) -> GatewayResult<owner::GetOwnerResult> {
        ctx.near_client()
            .owner(request.contract_id)
            .own_get_owner(())
            .await
            .map(|owner| owner::GetOwnerResult { owner })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<owner::GetProposedOwner, C> for Dispatch {
    async fn dispatch(
        request: owner::GetProposedOwner,
        ctx: C,
    ) -> GatewayResult<owner::GetProposedOwnerResult> {
        ctx.near_client()
            .owner(request.contract_id)
            .own_get_proposed_owner(())
            .await
            .map(|proposed_owner| owner::GetProposedOwnerResult { proposed_owner })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<owner::ProposeOwner, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<owner::ProposeOwner>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .owner(body.contract_id)
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
impl<C: HasNearClient> PlanWrite<owner::AcceptOwner, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<owner::AcceptOwner>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .owner(request.body.contract_id)
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
impl<C: HasNearClient> PlanWrite<owner::RenounceOwner, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<owner::RenounceOwner>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .owner(request.body.contract_id)
            .own_renounce_owner(
                ContractWriteOptions::new(request.signer_account_id)
                    .one_yocto()
                    .tgas(300),
                (),
            )
            .map(OperationPlan::from)
    }
}
