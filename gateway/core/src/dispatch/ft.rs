use futures::future::BoxFuture;
use templar_gateway_types::ft;

use crate::{
    client::{
        ft::{GetBalanceOfArgs, TransferArgs, TransferCallArgs},
        ContractWriteOptions,
    },
    dispatch::single_transaction_plan,
    operation::OperationPlan,
    GatewayContext, GatewayResult,
};
use crate::{DispatchRead, PlanWrite};

impl DispatchRead<GatewayContext> for ft::GetBalanceOf {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let balance = ctx
                .near()
                .ft(request.params.contract_id)
                .ft_balance_of(GetBalanceOfArgs {
                    account_id: request.params.account_id,
                })
                .await?;

            Ok(ft::GetBalanceOfResult { balance })
        })
    }
}

impl PlanWrite<GatewayContext> for ft::Transfer {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            Ok(single_transaction_plan(
                ctx.near().ft(request.body.contract_id).ft_transfer(
                    ContractWriteOptions::new(request.signer_account_id)
                        .gas(templar_gateway_types::NearGas::from_tgas(100))
                        .deposit(templar_gateway_types::NearToken::from_yoctonear(1)),
                    TransferArgs {
                        receiver_id: request.body.receiver_id,
                        amount: request.body.amount,
                        memo: request.body.memo,
                    },
                )?,
            ))
        })
    }
}

impl PlanWrite<GatewayContext> for ft::TransferCall {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            Ok(single_transaction_plan(
                ctx.near().ft(request.body.contract_id).ft_transfer_call(
                    ContractWriteOptions::new(request.signer_account_id)
                        .gas(templar_gateway_types::NearGas::from_tgas(100))
                        .deposit(templar_gateway_types::NearToken::from_yoctonear(1)),
                    TransferCallArgs {
                        receiver_id: request.body.receiver_id,
                        amount: request.body.amount,
                        memo: request.body.memo,
                        msg: request.body.msg,
                    },
                )?,
            ))
        })
    }
}
