use std::sync::Arc;

use blockchain_gateway_core::ft;
use futures::future::BoxFuture;

use crate::{
    actor::{operation_outcome_from_transaction_result, DispatchRead, DispatchWrite},
    client::{
        ft::{GetBalanceOfArgs, TransferArgs},
        ContractWriteOptions,
    },
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

impl DispatchWrite for ft::Transfer {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = ctx
                .ft(body.contract_id)
                .ft_transfer(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(100))
                        .deposit(blockchain_gateway_core::NearToken::from_yoctonear(1)),
                    TransferArgs {
                        receiver_id: body.receiver_id,
                        amount: body.amount,
                    },
                )
                .await?;

            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}
