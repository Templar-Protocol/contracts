use async_trait::async_trait;
use templar_gateway_types::{token, MethodSpec};

use super::Dispatch;
use crate::{operation::OperationPlan, DispatchRead, GatewayResult, HasNearClient, PlanWrite};

#[async_trait]
impl<C: HasNearClient> DispatchRead<token::GetBalanceOf, C> for Dispatch {
    async fn dispatch(
        request: <token::GetBalanceOf as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<token::GetBalanceOfResult> {
        let params = request.params;
        let balance = match params.token {
            token::TokenReference::Ft { contract_id } => {
                ctx.near_client()
                    .ft(contract_id)
                    .ft_balance_of(crate::client::ft::GetBalanceOfArgs {
                        account_id: params.account_id,
                    })
                    .await?
            }
            token::TokenReference::Mt {
                contract_id,
                token_id,
            } => {
                ctx.near_client()
                    .mt(contract_id)
                    .mt_balance_of(crate::client::mt::GetBalanceOfArgs {
                        account_id: params.account_id,
                        token_id,
                    })
                    .await?
            }
        };
        Ok(token::GetBalanceOfResult { balance })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<token::Transfer, C> for Dispatch {
    async fn plan(
        request: <token::Transfer as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let token::TransferBody {
            token,
            receiver_id,
            amount,
            memo,
        } = request.body;
        let transaction = match token {
            token::TokenReference::Ft { contract_id } => {
                ctx.near_client().ft(contract_id).ft_transfer(
                    crate::client::ContractWriteOptions::new(request.signer_account_id)
                        .gas(templar_gateway_types::NearGas::from_tgas(100))
                        .one_yocto(),
                    crate::client::ft::TransferArgs {
                        receiver_id,
                        amount,
                        memo,
                    },
                )?
            }
            token::TokenReference::Mt {
                contract_id,
                token_id,
            } => ctx.near_client().mt(contract_id).mt_transfer(
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
        Ok(OperationPlan::single(transaction))
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<token::TransferCall, C> for Dispatch {
    async fn plan(
        request: <token::TransferCall as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let token::TransferCallBody {
            token,
            receiver_id,
            amount,
            msg,
            memo,
        } = request.body;
        let transaction = match token {
            token::TokenReference::Ft { contract_id } => {
                ctx.near_client().ft(contract_id).ft_transfer_call(
                    crate::client::ContractWriteOptions::new(request.signer_account_id)
                        .gas(templar_gateway_types::NearGas::from_tgas(300))
                        .one_yocto(),
                    crate::client::ft::TransferCallArgs {
                        receiver_id,
                        amount,
                        memo,
                        msg,
                    },
                )?
            }
            token::TokenReference::Mt {
                contract_id,
                token_id,
            } => ctx.near_client().mt(contract_id).mt_transfer_call(
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
        Ok(OperationPlan::single(transaction))
    }
}
