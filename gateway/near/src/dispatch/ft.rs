use blockchain_gateway_core::ft;
use futures::future::BoxFuture;

use crate::{
    actor::{DispatchRead, PlanWrite},
    client::{
        ft::{GetBalanceOfArgs, TransferArgs},
        ContractWriteOptions,
    },
    dispatch::single_transaction_plan,
    operation::OperationPlan,
    GatewayContext, GatewayResult,
};

impl DispatchRead for ft::GetBalanceOf {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let balance = ctx
                .ft(request.params.contract_id)
                .ft_balance_of(GetBalanceOfArgs {
                    account_id: request.params.account_id,
                })
                .await?;

            Ok(ft::GetBalanceOfResult { balance })
        })
    }
}

impl PlanWrite for ft::Transfer {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            Ok(single_transaction_plan(
                ctx.ft(request.body.contract_id).ft_transfer(
                    ContractWriteOptions::new(request.signer_account_id)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(100))
                        .deposit(blockchain_gateway_core::NearToken::from_yoctonear(1)),
                    TransferArgs {
                        receiver_id: request.body.receiver_id,
                        amount: request.body.amount,
                    },
                )?,
            ))
        })
    }
}
