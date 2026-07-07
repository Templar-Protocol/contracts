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

#[cfg(test)]
mod tests {
    use super::*;
    use near_api::{CryptoHash, SecretKey};
    use templar_gateway_methods_spec::intents::{IntentPayload, SignedIntentPayload};
    use templar_gateway_types::{
        common::WriteRequest,
        primitive::{PublicKey, Signature},
        ManagedAccountId,
    };

    // A fixed, valid ED25519 key — only used to produce a well-formed (arbitrary)
    // signature/public key for the payload; the dispatch plan doesn't verify it.
    const TEST_KEY: &str =
        "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q";

    #[tokio::test]
    async fn plans_execute_intents_function_call() {
        let key: SecretKey = TEST_KEY.parse().unwrap();
        let signed = SignedIntentPayload {
            payload: IntentPayload {
                message: r#"{"intents":[]}"#.to_owned(),
                nonce: "bm9uY2U=".to_owned(),
                recipient: "intents.near".parse().unwrap(),
                callback_url: None,
            },
            standard: "nep413".to_owned(),
            signature: Signature(key.sign(CryptoHash([0u8; 32]))),
            public_key: PublicKey(key.public_key()),
        };

        let request = WriteRequest {
            signer_account_id: ManagedAccountId("treasury.near".parse().unwrap()),
            idempotency_key: None,
            body: intents::ExecuteIntents {
                contract_id: "intents.near".parse().unwrap(),
                signed: vec![signed],
            },
        };

        let plan = <Dispatch as PlanWrite<intents::ExecuteIntents, ()>>::plan(request, ())
            .await
            .expect("plan");

        assert_eq!(plan.steps.len(), 1);
        let step = &plan.steps[0];
        assert_eq!(step.receiver_id.as_str(), "intents.near");
        assert_eq!(step.actions.len(), 1);

        match &step.actions[0] {
            Action::FunctionCall(call) => {
                assert_eq!(call.method_name, "execute_intents");
                assert_eq!(call.gas, NearGas::from_tgas(100));
                assert_eq!(call.deposit, NearToken::from_yoctonear(0));

                // On-chain args are exactly `{ "signed": [...] }` — `contract_id`
                // only selects the receiver and must not leak into the payload.
                let args: serde_json::Value = serde_json::from_slice(&call.args).unwrap();
                assert!(args.get("contract_id").is_none());
                assert_eq!(args["signed"].as_array().unwrap().len(), 1);
                assert_eq!(args.as_object().unwrap().len(), 1);
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }
}
