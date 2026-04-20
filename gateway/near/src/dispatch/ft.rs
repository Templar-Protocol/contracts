use blockchain_gateway_core::ft;
use futures::future::BoxFuture;
use near_api::types::transaction::actions::{Action, FunctionCallAction};

use crate::{
    actor::{DispatchRead, DispatchWrite},
    client::ft::GetBalanceOfArgs,
    operation::{OperationPlan, PlannedTransaction},
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
    fn uses_operation_planning() -> bool {
        true
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }

    fn idempotency_key(request: &Self::Input) -> Option<&blockchain_gateway_core::IdempotencyKey> {
        request.idempotency_key.as_ref()
    }

    fn plan(
        request: Self::Input,
        _context: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            Ok(OperationPlan {
                wait_until: request.wait_until,
                steps: vec![PlannedTransaction {
                    signer_account_id: request.signer_account_id,
                    receiver_id: request.body.contract_id,
                    actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                        method_name: "ft_transfer".to_owned(),
                        args: serde_json::to_vec(&serde_json::json!({
                            "receiver_id": request.body.receiver_id,
                            "amount": request.body.amount.0.to_string(),
                        }))?,
                        gas: blockchain_gateway_core::NearGas::from_tgas(100),
                        deposit: blockchain_gateway_core::NearToken::from_yoctonear(1),
                    }))],
                }],
            })
        })
    }
}
