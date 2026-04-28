use futures::future::BoxFuture;
use templar_gateway_types::mt;

use crate::{
    client::{
        mt::{
            Approval, GetBalanceOfArgs, GetBatchBalanceOfArgs, GetBatchSupplyArgs, GetSupplyArgs,
            TransferArgs, TransferCallArgs,
        },
        ContractWriteOptions,
    },
    dispatch::single_transaction_plan,
    operation::OperationPlan,
    GatewayContext, GatewayResult,
};
use crate::{DispatchRead, PlanWrite};

fn approval(approval: Option<mt::MtApproval>) -> Option<Approval> {
    approval.map(|approval| Approval {
        owner_id: approval.owner_id,
        approval_id: approval.approval_id,
    })
}

impl DispatchRead<GatewayContext> for mt::GetBalanceOf {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let balance = ctx
                .near()
                .mt(params.contract_id)
                .mt_balance_of(GetBalanceOfArgs {
                    account_id: params.account_id,
                    token_id: params.token_id,
                })
                .await?;
            Ok(mt::GetBalanceOfResult { balance })
        })
    }
}

impl DispatchRead<GatewayContext> for mt::GetBatchBalanceOf {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let token_ids = params.token_ids;
            let values = ctx
                .near()
                .mt(params.contract_id)
                .mt_batch_balance_of(GetBatchBalanceOfArgs {
                    account_id: params.account_id,
                    token_ids: token_ids.clone(),
                })
                .await?;
            Ok(mt::GetBatchBalanceOfResult {
                balances: token_ids
                    .into_iter()
                    .zip(values)
                    .map(|(token_id, balance)| mt::BalanceEntry { token_id, balance })
                    .collect(),
            })
        })
    }
}

impl DispatchRead<GatewayContext> for mt::GetSupply {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let supply = ctx
                .near()
                .mt(params.contract_id)
                .mt_supply(GetSupplyArgs {
                    token_id: params.token_id,
                })
                .await?;
            Ok(mt::GetSupplyResult { supply })
        })
    }
}

impl DispatchRead<GatewayContext> for mt::GetBatchSupply {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let token_ids = params.token_ids;
            let values = ctx
                .near()
                .mt(params.contract_id)
                .mt_batch_supply(GetBatchSupplyArgs {
                    token_ids: token_ids.clone(),
                })
                .await?;
            Ok(mt::GetBatchSupplyResult {
                supplies: token_ids
                    .into_iter()
                    .zip(values)
                    .map(|(token_id, supply)| mt::SupplyEntry { token_id, supply })
                    .collect(),
            })
        })
    }
}

impl PlanWrite<GatewayContext> for mt::Transfer {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            Ok(single_transaction_plan(
                ctx.near().mt(body.contract_id).mt_transfer(
                    ContractWriteOptions::new(request.signer_account_id)
                        .gas(templar_gateway_types::NearGas::from_tgas(100))
                        .one_yocto(),
                    TransferArgs {
                        receiver_id: body.receiver_id,
                        token_id: body.token_id,
                        amount: body.amount,
                        approval: approval(body.approval),
                        memo: body.memo,
                    },
                )?,
            ))
        })
    }
}

impl PlanWrite<GatewayContext> for mt::TransferCall {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            Ok(single_transaction_plan(
                ctx.near().mt(body.contract_id).mt_transfer_call(
                    ContractWriteOptions::new(request.signer_account_id)
                        .gas(templar_gateway_types::NearGas::from_tgas(300))
                        .one_yocto(),
                    TransferCallArgs {
                        receiver_id: body.receiver_id,
                        token_id: body.token_id,
                        amount: body.amount,
                        approval: approval(body.approval),
                        memo: body.memo,
                        msg: body.msg,
                    },
                )?,
            ))
        })
    }
}
