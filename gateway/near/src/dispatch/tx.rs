use blockchain_gateway_core::tx;
use futures::future::BoxFuture;
use near_api::types::transaction::actions::{Action, FunctionCallAction};

use crate::{
    actor::{DispatchRead, PlanWrite},
    operation::{OperationPlan, PlannedTransaction},
    GatewayContext, GatewayResult,
};

impl DispatchRead for tx::Get {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let result = ctx
                .chain()
                .get_transaction(
                    request.params.tx_hash.into(),
                    request.params.sender_account_id,
                    request.params.wait_until.unwrap_or_default().into(),
                )
                .await?;

            Ok(tx::GetResult {
                status: if result.is_success() {
                    tx::Status::Succeeded
                } else if result.is_pending() {
                    tx::Status::Pending
                } else {
                    tx::Status::Failed
                },
                total_gas_burnt: result.total_gas_burnt,
                logs: result.logs().into_iter().map(ToString::to_string).collect(),
                return_value: match request.params.encoding {
                    tx::ValueEncoding::Json => result.json().ok().map(tx::ReturnValue::Json),
                    tx::ValueEncoding::Base64 => result
                        .raw_bytes()
                        .ok()
                        .map(|b| tx::ReturnValue::Base64(b.into())),
                },
            })
        })
    }
}

impl PlanWrite for tx::FunctionCall {
    fn plan(
        request: Self::Input,
        _context: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            Ok(OperationPlan {
                wait_until: request.wait_until,
                steps: vec![PlannedTransaction {
                    signer_account_id: request.signer_account_id,
                    receiver_id: request.body.receiver_id,
                    actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                        method_name: request.body.method_name.0,
                        args: request.body.args.try_into_bytes()?,
                        gas: request.body.gas,
                        deposit: request.body.deposit,
                    }))],
                }],
            })
        })
    }
}
