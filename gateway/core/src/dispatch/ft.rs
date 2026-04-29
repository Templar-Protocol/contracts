use async_trait::async_trait;
use templar_gateway_types::{ft, MethodSpec};

use super::Dispatch;
use crate::{
    client::{
        ft::{GetBalanceOfArgs, TransferArgs, TransferCallArgs},
        ContractWriteOptions,
    },
    operation::OperationPlan,
    DispatchRead, GatewayResult, HasNearClient, PlanWrite,
};

#[async_trait]
impl<C> DispatchRead<ft::GetBalanceOf, C> for Dispatch
where
    C: HasNearClient,
{
    async fn dispatch(
        request: <ft::GetBalanceOf as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<ft::GetBalanceOfResult> {
        let balance = ctx
            .near_client()
            .ft(request.params.contract_id)
            .ft_balance_of(GetBalanceOfArgs {
                account_id: request.params.account_id,
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
        request: <ft::Transfer as MethodSpec>::Input,
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
        request: <ft::TransferCall as MethodSpec>::Input,
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
