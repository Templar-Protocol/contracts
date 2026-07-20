use async_trait::async_trait;
use near_account_id::AccountId;
use templar_gateway_core::{
    client::{owner::OwnerProposeArgs, ContractWriteOptions},
    DispatchRead, GatewayError, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::owner;
use templar_gateway_types::ManagedAccountId;

use crate::Dispatch;

fn ensure_owner_signer(
    signer_account_id: &ManagedAccountId,
    contract_id: &AccountId,
    expected_owner: Option<&AccountId>,
    required_role: &'static str,
) -> GatewayResult<()> {
    if expected_owner == Some(&signer_account_id.0) {
        return Ok(());
    }

    Err(GatewayError::RequestPreconditionFailed(format!(
        "signer {} is not the {required_role} of contract {contract_id}",
        signer_account_id.0
    )))
}

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
        let signer_account_id = request.signer_account_id;
        let current_owner = ctx
            .near_client()
            .owner(body.contract_id.clone())
            .own_get_owner(())
            .await?;
        ensure_owner_signer(
            &signer_account_id,
            &body.contract_id,
            current_owner.as_ref(),
            "current owner",
        )?;

        ctx.near_client()
            .owner(body.contract_id)
            .own_propose_owner(
                ContractWriteOptions::new(signer_account_id)
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
        let contract_id = request.body.contract_id;
        let signer_account_id = request.signer_account_id;
        let proposed_owner = ctx
            .near_client()
            .owner(contract_id.clone())
            .own_get_proposed_owner(())
            .await?;
        ensure_owner_signer(
            &signer_account_id,
            &contract_id,
            proposed_owner.as_ref(),
            "proposed owner",
        )?;

        ctx.near_client()
            .owner(contract_id)
            .own_accept_owner(
                ContractWriteOptions::new(signer_account_id)
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
        let contract_id = request.body.contract_id;
        let signer_account_id = request.signer_account_id;
        let current_owner = ctx
            .near_client()
            .owner(contract_id.clone())
            .own_get_owner(())
            .await?;
        ensure_owner_signer(
            &signer_account_id,
            &contract_id,
            current_owner.as_ref(),
            "current owner",
        )?;

        ctx.near_client()
            .owner(contract_id)
            .own_renounce_owner(
                ContractWriteOptions::new(signer_account_id)
                    .one_yocto()
                    .tgas(300),
                (),
            )
            .map(OperationPlan::from)
    }
}
