use futures::future::BoxFuture;
use templar_gateway_types::ft;

use crate::{
    client::{
        ft::{GetBalanceOfArgs, TransferArgs, TransferCallArgs},
        ContractWriteOptions,
    },
    operation::OperationPlan,
    GatewayResult, HasNearClient,
};
use crate::{DispatchRead, PlanWrite};

impl<C: HasNearClient> DispatchRead<C> for ft::GetBalanceOf {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let balance = ctx
                .near_client()
                .ft(request.params.contract_id)
                .ft_balance_of(GetBalanceOfArgs {
                    account_id: request.params.account_id,
                })
                .await?;

            Ok(ft::GetBalanceOfResult { balance })
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for ft::Transfer {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
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
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for ft::TransferCall {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
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
        })
    }
}
