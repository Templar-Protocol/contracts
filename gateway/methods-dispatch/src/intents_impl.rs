use async_trait::async_trait;
use near_api::types::transaction::actions::{Action, FunctionCallAction};
use templar_gateway_core::{GatewayResult, OperationPlan, PlanWrite};
use templar_gateway_methods_spec::intents;
use templar_gateway_types::{NearGas, NearToken};

use crate::Dispatch;

#[async_trait]
impl<C: Send + 'static> PlanWrite<intents::ExecuteIntents, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<intents::ExecuteIntents>,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
        // The on-chain `execute_intents` args are `{ signed: [...] }`; the
        // `contract_id` only picks the receiver and is not part of the payload.
        let args = serde_json::to_vec(&serde_json::json!({ "signed": request.body.signed }))?;

        Ok(OperationPlan::execute(
            request.signer_account_id,
            request.body.contract_id,
            vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: "execute_intents".to_owned(),
                args,
                gas: NearGas::from_tgas(100),
                deposit: NearToken::from_yoctonear(0),
            }))],
        ))
    }
}
