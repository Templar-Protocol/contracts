use async_trait::async_trait;
use near_api::types::transaction::actions::{
    Action, DeployContractAction, FunctionCallAction, TransferAction,
};
use templar_gateway_core::{
    DispatchRead, GatewayError, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::tx;
use templar_gateway_types::{
    protocol::{MAX_ACTIONS_PER_RECEIPT, MAX_TOTAL_PREPAID_GAS, MAX_TRANSACTION_SIZE},
    ActionInput, NearGas,
};

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<tx::Get, C> for Dispatch {
    async fn dispatch(request: tx::Get, ctx: C) -> GatewayResult<tx::GetResult> {
        let result = ctx
            .near_client()
            .chain()
            .get_transaction(
                request.tx_hash.into(),
                request.sender_account_id,
                request.wait_until.unwrap_or_default().into(),
            )
            .await?;

        // Sum tokens burnt across the transaction and all receipts (the signer's
        // true cost) before consuming `result` for the return value below.
        let tokens_burnt = result
            .outcomes()
            .iter()
            .map(|outcome| outcome.tokens_burnt)
            .fold(near_api::NearToken::from_yoctonear(0), |acc, item| {
                acc.saturating_add(item)
            });

        Ok(tx::GetResult {
            status: if result.is_success() {
                tx::Status::Succeeded
            } else if result.is_pending() {
                tx::Status::Pending
            } else {
                tx::Status::Failed
            },
            total_gas_burnt: result.total_gas_burnt,
            tokens_burnt,
            logs: result.logs().into_iter().map(ToString::to_string).collect(),
            // The distinct contracts whose receipts failed, even when the
            // transaction's final status is success (e.g. a refunded
            // `ft_transfer_call`). Deduped: the set of failing contracts is the
            // logical signal, independent of how many receipts each produced.
            failed_receipts: result
                .receipt_failures()
                .iter()
                .map(|outcome| outcome.executor_id.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            return_value: match request.encoding {
                tx::ValueEncoding::Json => result.json().ok().map(tx::ReturnValue::Json),
                tx::ValueEncoding::Base64 => result
                    .raw_bytes()
                    .ok()
                    .map(|b| tx::ReturnValue::Base64(b.into())),
            },
        })
    }
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<tx::FunctionCall, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<tx::FunctionCall>,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
        Ok(OperationPlan::execute(
            request.signer_account_id,
            request.body.receiver_id,
            vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: request.body.method_name.0,
                args: request.body.args.try_into_bytes()?,
                gas: request.body.gas,
                deposit: request.body.deposit,
            }))],
        ))
    }
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<tx::Transfer, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<tx::Transfer>,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
        Ok(OperationPlan::execute(
            request.signer_account_id,
            request.body.receiver_id,
            vec![Action::Transfer(TransferAction {
                deposit: request.body.amount,
            })],
        ))
    }
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<tx::RelaySignedDelegateAction, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<tx::RelaySignedDelegateAction>,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
        // NEP-366: the relayer wraps the user's signed delegate action in a
        // transaction it signs and pays for, sent to the delegate's sender. The
        // payload was already borsh-decoded + validated at the spec boundary.
        let signed_delegate_action = request.body.signed_delegate_action.into_inner();

        Ok(OperationPlan::execute(
            request.signer_account_id,
            signed_delegate_action.delegate_action.sender_id.clone(),
            vec![Action::Delegate(Box::new(signed_delegate_action))],
        ))
    }
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<tx::DeployContract, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<tx::DeployContract>,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
        Ok(OperationPlan::execute(
            request.signer_account_id,
            request.body.account_id,
            vec![Action::DeployContract(DeployContractAction {
                code: request.body.code.0,
            })],
        ))
    }
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<tx::DeployAndInit, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<tx::DeployAndInit>,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
        Ok(OperationPlan::execute(
            request.signer_account_id,
            request.body.account_id,
            vec![
                Action::DeployContract(DeployContractAction {
                    code: request.body.code.0,
                }),
                Action::FunctionCall(Box::new(FunctionCallAction {
                    method_name: request.body.method_name.0,
                    args: request.body.args.try_into_bytes()?,
                    gas: request.body.gas,
                    deposit: request.body.deposit,
                })),
            ],
        ))
    }
}

/// Signer, key, nonce, receiver, block hash and signature, rounded up.
const SIGNED_ENVELOPE_BYTES: usize = 512;

/// `None` on overflow, which is over the limit either way.
fn total_prepaid_gas(actions: &[ActionInput]) -> Option<NearGas> {
    actions
        .iter()
        .try_fold(0u64, |total, action| match action {
            ActionInput::FunctionCall { gas, .. } => total.checked_add(gas.as_gas()),
            _ => Some(total),
        })
        .map(NearGas::from_gas)
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<tx::Batch, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<tx::Batch>,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
        // A rejected submission strands its step in `Submitted`, so every limit
        // is checked before the operation is persisted.
        let count = request.body.actions.len();
        if count == 0 {
            return Err(GatewayError::RequestPreconditionFailed(
                "a batch must carry at least one action".to_owned(),
            ));
        }
        if count > MAX_ACTIONS_PER_RECEIPT {
            return Err(GatewayError::RequestPreconditionFailed(format!(
                "a batch carries at most {MAX_ACTIONS_PER_RECEIPT} actions, got {count}"
            )));
        }

        match total_prepaid_gas(&request.body.actions) {
            Some(total) if total <= MAX_TOTAL_PREPAID_GAS => {}
            Some(total) => {
                return Err(GatewayError::RequestPreconditionFailed(format!(
                    "a batch prepays at most {} gas across its actions, got {}",
                    MAX_TOTAL_PREPAID_GAS.as_gas(),
                    total.as_gas()
                )))
            }
            None => {
                return Err(GatewayError::RequestPreconditionFailed(format!(
                    "a batch prepays at most {} gas across its actions; this one overflows u64",
                    MAX_TOTAL_PREPAID_GAS.as_gas()
                )))
            }
        }

        let actions = request
            .body
            .actions
            .into_iter()
            .map(Action::try_from)
            .collect::<Result<Vec<_>, _>>()?;

        // Bounds the signed size rather than computing it: the envelope is only
        // known once a nonce and block hash exist, and payloads dominate anyway.
        let size = borsh::to_vec(&actions)?.len() + SIGNED_ENVELOPE_BYTES;
        if size > MAX_TRANSACTION_SIZE {
            return Err(GatewayError::RequestPreconditionFailed(format!(
                "a batch serializes to at most {MAX_TRANSACTION_SIZE} bytes of \
                 transaction size, got about {size}"
            )));
        }

        Ok(OperationPlan::execute(
            request.signer_account_id,
            request.body.receiver_id,
            actions,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use templar_gateway_types::{
        common::{ContractArgs, WriteRequest},
        ActionInput, Base64Bytes, ContractMethodName, CryptoHash, GlobalContractIdentifierInput,
        ManagedAccountId, NearGas, NearToken,
    };

    fn request(actions: Vec<ActionInput>) -> WriteRequest<tx::Batch> {
        WriteRequest {
            signer_account_id: ManagedAccountId("signer.near".parse().expect("valid account id")),
            idempotency_key: None,
            body: tx::Batch {
                receiver_id: "target.near".parse().expect("valid account id"),
                actions,
            },
        }
    }

    fn call(gas: NearGas) -> ActionInput {
        ActionInput::FunctionCall {
            method_name: ContractMethodName("noop".to_owned()),
            args: ContractArgs::Raw(Base64Bytes(Vec::new())),
            gas,
            deposit: NearToken::from_yoctonear(0),
        }
    }

    fn transfer() -> ActionInput {
        ActionInput::Transfer {
            deposit: NearToken::from_yoctonear(1),
        }
    }

    async fn plan(actions: Vec<ActionInput>) -> GatewayResult<OperationPlan> {
        <Dispatch as PlanWrite<tx::Batch, ()>>::plan(request(actions), ()).await
    }

    #[tokio::test]
    async fn plans_one_transaction_carrying_every_action_in_order() {
        let plan = plan(vec![
            ActionInput::DeployContract {
                code: Base64Bytes(vec![0, 97, 115, 109]),
            },
            ActionInput::FunctionCall {
                method_name: ContractMethodName("migrate".to_owned()),
                args: ContractArgs::Raw(Base64Bytes(vec![1, 2])),
                gas: NearGas::from_tgas(30),
                deposit: NearToken::from_yoctonear(0),
            },
            ActionInput::UseGlobalContract {
                contract_identifier: GlobalContractIdentifierInput::CodeHash(CryptoHash(
                    near_api::CryptoHash([7u8; 32]),
                )),
            },
        ])
        .await
        .expect("plan");

        assert_eq!(plan.steps.len(), 1, "a batch is one transaction");
        let step = &plan.steps[0];
        assert_eq!(step.signer_account_id.0.as_str(), "signer.near");
        assert_eq!(step.receiver_id.as_str(), "target.near");

        match step.actions.as_slice() {
            [Action::DeployContract(deploy), Action::FunctionCall(call), Action::UseGlobalContract(global)] =>
            {
                assert_eq!(deploy.code, vec![0, 97, 115, 109]);
                assert_eq!(call.method_name, "migrate");
                assert_eq!(call.args, vec![1, 2]);
                assert_eq!(
                    global.contract_identifier,
                    near_api::types::transaction::actions::GlobalContractIdentifier::CodeHash(
                        near_api::CryptoHash([7u8; 32])
                    )
                );
            }
            other => panic!("unexpected actions: {other:?}"),
        }
    }

    #[tokio::test]
    async fn encodes_json_function_call_args() {
        let plan = plan(vec![ActionInput::FunctionCall {
            method_name: ContractMethodName("set".to_owned()),
            args: ContractArgs::Json(serde_json::json!({ "value": 1 })),
            gas: NearGas::from_tgas(10),
            deposit: NearToken::from_yoctonear(0),
        }])
        .await
        .expect("plan");

        match plan.steps[0].actions.as_slice() {
            [Action::FunctionCall(call)] => {
                assert_eq!(call.args, br#"{"value":1}"#.to_vec());
            }
            other => panic!("unexpected actions: {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_an_empty_batch() {
        let error = plan(vec![])
            .await
            .expect_err("empty batch must be rejected");
        assert!(
            matches!(error, GatewayError::RequestPreconditionFailed(ref message)
                if message.contains("at least one action")),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn rejects_more_actions_than_a_receipt_holds() {
        let accepted = plan(vec![transfer(); MAX_ACTIONS_PER_RECEIPT])
            .await
            .expect("the limit itself is allowed");
        assert_eq!(accepted.steps[0].actions.len(), MAX_ACTIONS_PER_RECEIPT);

        let error = plan(vec![transfer(); MAX_ACTIONS_PER_RECEIPT + 1])
            .await
            .expect_err("over the limit must be rejected");
        assert!(
            matches!(error, GatewayError::RequestPreconditionFailed(ref message)
                if message.contains("at most 100 actions, got 101")),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn rejects_a_batch_whose_actions_together_prepay_too_much_gas() {
        let half = NearGas::from_gas(MAX_TOTAL_PREPAID_GAS.as_gas() / 2);
        plan(vec![call(half), call(half)])
            .await
            .expect("the limit itself is allowed");

        // Each action is individually valid; only the sum is not.
        let error = plan(vec![call(half), call(half), call(NearGas::from_gas(1))])
            .await
            .expect_err("over the total must be rejected");
        assert!(
            matches!(error, GatewayError::RequestPreconditionFailed(ref message)
                if message.contains("prepays at most")),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn rejects_a_batch_whose_gas_sum_overflows() {
        let error = plan(vec![
            call(NearGas::from_gas(u64::MAX)),
            call(NearGas::from_gas(1)),
        ])
        .await
        .expect_err("an overflowing sum must be rejected, not wrapped");
        assert!(
            matches!(error, GatewayError::RequestPreconditionFailed(ref message)
                if message.contains("overflows")),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn non_function_call_actions_prepay_no_gas() {
        plan(vec![transfer(); MAX_ACTIONS_PER_RECEIPT])
            .await
            .expect("transfers prepay nothing, so no batch of them can exceed the gas limit");
    }

    #[tokio::test]
    async fn rejects_a_batch_whose_actions_together_exceed_the_transaction_size() {
        let deploy = |len: usize| ActionInput::DeployContract {
            code: Base64Bytes(vec![0u8; len]),
        };
        let half = MAX_TRANSACTION_SIZE / 2;

        plan(vec![deploy(half - 4096), deploy(half - 4096)])
            .await
            .expect("two deploys that fit together are allowed");

        // Each blob is individually deployable; only together do they overflow.
        let error = plan(vec![deploy(half), deploy(half)])
            .await
            .expect_err("over the transaction size must be rejected");
        assert!(
            matches!(error, GatewayError::RequestPreconditionFailed(ref message)
                if message.contains("transaction size")),
            "unexpected error: {error}"
        );
    }
}
