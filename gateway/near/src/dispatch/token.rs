use futures::future::BoxFuture;
use templar_gateway_types::token;

use crate::{
    actor::{DispatchRead, PlanWrite},
    dispatch::single_transaction_plan,
    operation::OperationPlan,
    GatewayContext, GatewayResult,
};

impl DispatchRead for token::GetBalanceOf {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let balance = match params.token {
                token::TokenReference::Ft { contract_id } => {
                    ctx.ft(contract_id)
                        .ft_balance_of(crate::client::ft::GetBalanceOfArgs {
                            account_id: params.account_id,
                        })
                        .await?
                }
                token::TokenReference::Mt {
                    contract_id,
                    token_id,
                } => {
                    ctx.mt(contract_id)
                        .mt_balance_of(crate::client::mt::GetBalanceOfArgs {
                            account_id: params.account_id,
                            token_id,
                        })
                        .await?
                }
            };
            Ok(token::GetBalanceOfResult { balance })
        })
    }
}

impl PlanWrite for token::Transfer {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let token::TransferBody {
                token,
                receiver_id,
                amount,
                memo,
            } = request.body;
            let transaction = match token {
                token::TokenReference::Ft { contract_id } => ctx.ft(contract_id).ft_transfer(
                    crate::client::ContractWriteOptions::new(request.signer_account_id)
                        .gas(templar_gateway_types::NearGas::from_tgas(100))
                        .one_yocto(),
                    crate::client::ft::TransferArgs {
                        receiver_id,
                        amount,
                        memo,
                    },
                )?,
                token::TokenReference::Mt {
                    contract_id,
                    token_id,
                } => ctx.mt(contract_id).mt_transfer(
                    crate::client::ContractWriteOptions::new(request.signer_account_id)
                        .gas(templar_gateway_types::NearGas::from_tgas(100))
                        .one_yocto(),
                    crate::client::mt::TransferArgs {
                        receiver_id,
                        token_id,
                        amount,
                        approval: None,
                        memo,
                    },
                )?,
            };
            Ok(single_transaction_plan(transaction))
        })
    }
}

impl PlanWrite for token::TransferCall {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let token::TransferCallBody {
                token,
                receiver_id,
                amount,
                msg,
                memo,
            } = request.body;
            let transaction = match token {
                token::TokenReference::Ft { contract_id } => ctx.ft(contract_id).ft_transfer_call(
                    crate::client::ContractWriteOptions::new(request.signer_account_id)
                        .gas(templar_gateway_types::NearGas::from_tgas(300))
                        .one_yocto(),
                    crate::client::ft::TransferCallArgs {
                        receiver_id,
                        amount,
                        memo,
                        msg,
                    },
                )?,
                token::TokenReference::Mt {
                    contract_id,
                    token_id,
                } => ctx.mt(contract_id).mt_transfer_call(
                    crate::client::ContractWriteOptions::new(request.signer_account_id)
                        .gas(templar_gateway_types::NearGas::from_tgas(300))
                        .one_yocto(),
                    crate::client::mt::TransferCallArgs {
                        receiver_id,
                        token_id,
                        amount,
                        approval: None,
                        memo,
                        msg,
                    },
                )?,
            };
            Ok(single_transaction_plan(transaction))
        })
    }
}
