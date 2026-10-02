use async_trait::async_trait;
use templar_gateway_core::{
    client::{
        mt::{
            Approval, GetBalanceOfArgs, GetBatchBalanceOfArgs, GetBatchSupplyArgs, GetSupplyArgs,
            TransferArgs, TransferCallArgs,
        },
        ContractWriteOptions,
    },
    DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::mt;
use templar_primitives::SU128;

use crate::Dispatch;

fn approval(approval: Option<mt::MtApproval>) -> Option<Approval> {
    approval.map(|approval| Approval {
        owner_id: approval.owner_id,
        approval_id: approval.approval_id,
    })
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<mt::GetBalanceOf, C> for Dispatch {
    async fn dispatch(request: mt::GetBalanceOf, ctx: C) -> GatewayResult<SU128> {
        let params = request;
        ctx.near_client()
            .mt(params.contract_id)
            .mt_balance_of(GetBalanceOfArgs {
                account_id: params.account_id,
                token_id: params.token_id,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<mt::GetBatchBalanceOf, C> for Dispatch {
    async fn dispatch(
        request: mt::GetBatchBalanceOf,
        ctx: C,
    ) -> GatewayResult<Vec<mt::BalanceEntry>> {
        let params = request;
        let token_ids = params.token_ids;
        let values = ctx
            .near_client()
            .mt(params.contract_id)
            .mt_batch_balance_of(GetBatchBalanceOfArgs {
                account_id: params.account_id,
                token_ids: token_ids.clone(),
            })
            .await?;
        Ok(token_ids
            .into_iter()
            .zip(values)
            .map(|(token_id, balance)| mt::BalanceEntry { token_id, balance })
            .collect())
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<mt::GetSupply, C> for Dispatch {
    async fn dispatch(request: mt::GetSupply, ctx: C) -> GatewayResult<Option<SU128>> {
        let params = request;
        ctx.near_client()
            .mt(params.contract_id)
            .mt_supply(GetSupplyArgs {
                token_id: params.token_id,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<mt::GetBatchSupply, C> for Dispatch {
    async fn dispatch(request: mt::GetBatchSupply, ctx: C) -> GatewayResult<Vec<mt::SupplyEntry>> {
        let params = request;
        let token_ids = params.token_ids;
        let values = ctx
            .near_client()
            .mt(params.contract_id)
            .mt_batch_supply(GetBatchSupplyArgs {
                token_ids: token_ids.clone(),
            })
            .await?;
        Ok(token_ids
            .into_iter()
            .zip(values)
            .map(|(token_id, supply)| mt::SupplyEntry { token_id, supply })
            .collect())
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<mt::Transfer, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<mt::Transfer>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .mt(body.contract_id)
            .mt_transfer(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(100)
                    .one_yocto(),
                TransferArgs {
                    receiver_id: body.receiver_id,
                    token_id: body.token_id,
                    amount: body.amount,
                    approval: approval(body.approval),
                    memo: body.memo,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<mt::TransferCall, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<mt::TransferCall>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .mt(body.contract_id)
            .mt_transfer_call(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(300)
                    .one_yocto(),
                TransferCallArgs {
                    receiver_id: body.receiver_id,
                    token_id: body.token_id,
                    amount: body.amount,
                    approval: approval(body.approval),
                    memo: body.memo,
                    msg: body.msg,
                },
            )
            .map(OperationPlan::from)
    }
}
