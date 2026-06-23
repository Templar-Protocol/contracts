use async_trait::async_trait;
use templar_gateway_core::{
    client::ft::{GetBalanceOfArgs, TransferArgs, TransferCallArgs},
    ContractWriteOptions, DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::ft;

use crate::Dispatch;

#[async_trait]
impl<C> DispatchRead<ft::GetBalanceOf, C> for Dispatch
where
    C: HasNearClient,
{
    async fn dispatch(request: ft::GetBalanceOf, ctx: C) -> GatewayResult<ft::GetBalanceOfResult> {
        let balance = ctx
            .near_client()
            .ft(request.contract_id)
            .ft_balance_of(GetBalanceOfArgs {
                account_id: request.account_id,
            })
            .await?;

        Ok(ft::GetBalanceOfResult { balance })
    }
}

#[async_trait]
impl<C> PlanWrite<ft::Transfer, C> for Dispatch
where
    C: HasNearClient,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<ft::Transfer>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .ft(request.body.contract_id)
            .ft_transfer(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(100)
                    .one_yocto(),
                TransferArgs {
                    receiver_id: request.body.receiver_id,
                    amount: request.body.amount,
                    memo: request.body.memo,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<ft::TransferCall, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<ft::TransferCall>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        ctx.near_client()
            .ft(request.body.contract_id)
            .ft_transfer_call(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(100)
                    .one_yocto(),
                TransferCallArgs {
                    receiver_id: request.body.receiver_id,
                    amount: request.body.amount,
                    memo: request.body.memo,
                    msg: request.body.msg,
                },
            )
            .map(OperationPlan::from)
    }
}
